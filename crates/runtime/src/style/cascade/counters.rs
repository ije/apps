use crate::css::{
    properties::Prop, properties::Style, value::counter::counter_text, value::tokens::components,
};
use crate::dom::Document;
use crate::style::cascade::TreeWalk;
use crate::style::cascade::computed::{ComputedStyles, DISPLAY_NONE};
use rustc_hash::FxHashMap;
use std::rc::Rc;

pub(crate) fn list_item(style: &Style) -> bool {
    style.get(Prop::DISPLAY) == Some("list-item")
}

pub(crate) fn listed(doc: &Document, id: usize) -> bool {
    matches!(doc.nodes[id].tag.as_str(), "ol" | "ul" | "menu" | "li")
}

// Numbers the list items and counters, or keeps the numbers of the last pass when none of their inputs changed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn number_lists(
    doc: &Document,
    walk: &TreeWalk,
    out: &mut ComputedStyles,
    previous: Option<&ComputedStyles>,
    _last: Option<&ComputedStyles>,
    fast: bool,
    recount: bool,
    counters: bool,
) {
    let ComputedStyles {
        values: styles,
        flags: style_flags,
        pseudos,
        list_items,
        counting,
        counted,
        ..
    } = out;
    let _count = doc.nodes.len();
    let reused = previous
        .filter(|previous| fast && previous.counted)
        .filter(|previous| {
            // Attributes like `start`, `reversed` and `value` can change without a new style.
            !recount
                && !doc.nodes.changed_since(previous.revision).any(|id| {
                    listed(doc, id)
                        || list_item(&styles[id])
                        || previous.values.get(id).is_some_and(|old| list_item(old))
                        || doc.nodes[id]
                            .children
                            .iter()
                            .any(|&child| list_item(&styles[child]))
                })
        });
    if let Some(previous) = reused {
        *counted = true;
        list_items.clone_from(&previous.list_items);
    } else if counters || previous.is_some_and(|previous| fast && previous.counted) {
        *counted = true;
        let list = |value: Option<&str>, default: i64| -> Vec<(String, i64)> {
            let parts = value.map_or_else(Vec::new, |value| components(value, ' '));
            let mut list: Vec<(String, i64)> = Vec::new();
            for part in parts {
                match (part.parse::<i64>(), list.last_mut()) {
                    (Ok(number), Some(last)) => last.1 = number,
                    (Ok(_), None) => {}
                    (Err(_), _) if part == "none" => {}
                    (Err(_), _) => list.push((part.to_owned(), default)),
                }
            }
            list
        };
        let mut counters: Vec<(String, i64, usize)> = Vec::new();
        let apply = |counters: &mut Vec<(String, i64, usize)>,
                     style: &Style,
                     parent: usize,
                     implicit: Option<i64>| {
            for (name, value) in list(style.get(Prop::COUNTER_RESET), 0) {
                // A reset replaces the counter of a preceding sibling.
                if let Some(at) = counters
                    .iter()
                    .rposition(|counter| counter.0 == name && counter.2 == parent)
                {
                    counters.remove(at);
                }
                counters.push((name, value, parent));
            }
            for (name, value) in list(style.get(Prop::COUNTER_SET), 0) {
                match counters.iter_mut().rfind(|counter| counter.0 == name) {
                    Some(counter) => counter.1 = value,
                    None => counters.push((name, value, parent)),
                }
            }
            let mut increments = list(style.get(Prop::COUNTER_INCREMENT), 1);
            if let Some(step) =
                implicit.filter(|_| !increments.iter().any(|(name, _)| name == "list-item"))
            {
                increments.push(("list-item".into(), step));
            }
            for (name, step) in increments {
                match counters.iter_mut().rfind(|counter| counter.0 == name) {
                    Some(counter) => counter.1 = counter.1.saturating_add(step),
                    None => counters.push((name, step, parent)),
                }
            }
        };
        let content = |counters: &[(String, i64, usize)], content: &str| -> Option<String> {
            if !content.contains("counter") {
                return None;
            }
            let parts: Vec<String> = components(content, ' ')
                .into_iter()
                .map(|part| {
                    let Some((function, arguments)) = part
                        .strip_suffix(')')
                        .and_then(|part| part.split_once('('))
                        .filter(|(function, _)| matches!(*function, "counter" | "counters"))
                    else {
                        return part.to_owned();
                    };
                    let arguments: Vec<&str> = components(arguments, ',')
                        .into_iter()
                        .map(str::trim)
                        .collect();
                    let name = arguments.first().copied().unwrap_or_default();
                    let (separator, kind) = if function == "counters" {
                        (
                            arguments
                                .get(1)
                                .map(|value| value.trim_matches(['"', '\'']))
                                .unwrap_or(""),
                            arguments.get(2),
                        )
                    } else {
                        ("", arguments.get(1))
                    };
                    let kind = kind.copied().unwrap_or("decimal");
                    let values: Vec<i64> = counters
                        .iter()
                        .filter(|counter| counter.0 == name)
                        .map(|counter| counter.1)
                        .collect();
                    let values = match (function, values.last()) {
                        ("counter", Some(&last)) => vec![last],
                        ("counter", None) => vec![0],
                        _ if values.is_empty() => vec![0],
                        _ => values,
                    };
                    serde_json::to_string(
                        &values
                            .into_iter()
                            .map(|value| counter_text(kind, value))
                            .collect::<Vec<_>>()
                            .join(separator),
                    )
                    .unwrap_or_default()
                })
                .collect();
            Some(parts.join(" "))
        };
        // A node that got no new style in a fast pass still holds the text, so its declared value is in
        // `counting`. A new style drops that entry.
        let number = |pseudos: &mut FxHashMap<usize, Rc<[Style; 2]>>,
                      counting: &mut FxHashMap<usize, [Option<Rc<str>>; 2]>,
                      counters: &[(String, i64, usize)],
                      id: usize,
                      side: usize| {
            let declared = counting.get(&id).and_then(|kept| kept[side].clone());
            let own = &pseudos[&id][side];
            let declared = declared.or_else(|| own.value(Prop::CONTENT).cloned());
            if let Some(text) = declared
                .as_deref()
                .and_then(|value| content(counters, value))
            {
                counting.entry(id).or_default()[side] = declared;
                if own.get(Prop::CONTENT) != Some(&text) {
                    Rc::make_mut(pseudos.get_mut(&id).unwrap())[side].insert(Prop::CONTENT, text);
                }
            }
        };
        for &(root, _) in walk.trees.iter() {
            counters.clear();
            let mut pending = vec![(root, false)];
            while let Some((id, exit)) = pending.pop() {
                let node = &doc.nodes[id];
                let parent = node.parent.unwrap_or(usize::MAX);
                if exit {
                    if let Some(own) = pseudos.get(&id).cloned() {
                        apply(&mut counters, &own[1], id, None);
                        number(pseudos, counting, &counters, id, 1);
                    }
                    counters.retain(|counter| counter.2 != id);
                    continue;
                }
                if style_flags[id] & DISPLAY_NONE != 0 || node.tag.starts_with('#') && id != root {
                    continue;
                }
                let style = styles[id].clone();
                // Like Chrome, lists reset `list-item` unless their `counter-reset` names it.
                if matches!(node.tag.as_str(), "ol" | "ul" | "menu")
                    && style
                        .get(Prop::COUNTER_RESET)
                        .is_none_or(|reset| !components(reset, ' ').contains(&"list-item"))
                {
                    let items = node
                        .children
                        .iter()
                        .filter(|&&child| styles[child].get(Prop::DISPLAY) == Some("list-item"))
                        .count() as i64;
                    let reversed = node.attrs.contains_key("reversed");
                    let start = node
                        .attrs
                        .get("start")
                        .and_then(|value| value.parse::<i64>().ok());
                    let first = if reversed {
                        start.unwrap_or(items).saturating_add(1)
                    } else {
                        start.unwrap_or(1).saturating_sub(1)
                    };
                    counters.push(("list-item".into(), first, parent));
                }
                let item = style.get(Prop::DISPLAY) == Some("list-item");
                let reversed = item
                    && node
                        .parent
                        .is_some_and(|parent| doc.nodes[parent].attrs.contains_key("reversed"));
                apply(
                    &mut counters,
                    &style,
                    parent,
                    item.then_some(if reversed { -1 } else { 1 }),
                );
                if item {
                    if let Some(value) = node
                        .attrs
                        .get("value")
                        .and_then(|value| value.parse::<i64>().ok())
                    {
                        match counters
                            .iter_mut()
                            .rfind(|counter| counter.0 == "list-item")
                        {
                            Some(counter) => counter.1 = value,
                            None => counters.push(("list-item".into(), value, parent)),
                        }
                    }
                    if let Some(counter) = counters.iter().rfind(|counter| counter.0 == "list-item")
                    {
                        list_items.insert(id, counter.1);
                    }
                }
                if let Some(own) = pseudos.get(&id).cloned() {
                    apply(&mut counters, &own[0], id, None);
                    number(pseudos, counting, &counters, id, 0);
                }
                pending.push((id, true));
                pending.extend(node.children.iter().rev().map(|&child| (child, false)));
            }
        }
    }
}
