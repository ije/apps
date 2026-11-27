use super::controls::radio;
use super::*;
use crate::css::selector::ACTIVE;
use crate::css::selector::FOCUS;
use crate::css::selector::FOCUS_VISIBLE;
use crate::css::selector::FOCUS_WITHIN;
use crate::css::selector::HOVER;
use crate::css::selector::Selector;
use crate::css::selector::SimpleSelector;
use crate::dom::Document;
use std::{hash::BuildHasher, rc::Rc};

// Whether a simple selector reads the place of the node among its siblings.
fn positional(simple: &SimpleSelector) -> bool {
    let SimpleSelector::Pseudo(name, _, parsed, ..) = simple else {
        return false;
    };
    match name.as_str() {
        "first-child" | "last-child" | "only-child" | "nth-child" | "nth-last-child"
        | "first-of-type" | "last-of-type" | "only-of-type" | "nth-of-type"
        | "nth-last-of-type" => true,
        "not" | "is" | "where" => parsed.iter().any(|s| s.simple.iter().any(positional)),
        _ => false,
    }
}

pub(super) fn next_stamp(stamp: &mut u32, stamps: &mut Vec<u32>, len: usize) -> u32 {
    *stamp = stamp.wrapping_add(1);
    if *stamp == 0 {
        stamps.fill(0);
        *stamp = 1;
    }
    if stamps.len() < len {
        stamps.resize(len, 0);
    }
    *stamp
}

// Whether an input has a range limitation, so `:in-range` or `:out-of-range` matches it.
pub(super) fn ranged(node: &crate::dom::Node) -> bool {
    node.tag == "input"
        && [
            "number",
            "range",
            "date",
            "month",
            "week",
            "time",
            "datetime-local",
        ]
        .iter()
        .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
        && (node.input_type().eq_ignore_ascii_case("range")
            || node.attrs.contains_key("min")
            || node.attrs.contains_key("max"))
}

// Match `disabled` in dom.js: a disabled fieldset disables the controls in it, except in its first legend,
// and a disabled optgroup its options.
pub(super) fn disabled(doc: &Document, id: usize, memo: &mut SelectorCache) -> bool {
    let tag = doc.nodes[id].tag.as_str();
    let (mut at, mut child) = (id, None);
    while !doc.nodes[at].tag.starts_with('#') {
        let node = &doc.nodes[at];
        memo.depend(at, 0);
        if node.attrs.contains_key("disabled") {
            if at == id || node.tag == "optgroup" && tag == "option" {
                return true;
            }
            if node.tag == "fieldset" {
                memo.depend(at, 2);
                let legend = node.children.iter().find(|&&kid| {
                    memo.depend(kid, 4);
                    doc.nodes[kid].tag == "legend"
                });
                if legend != child.as_ref() {
                    return true;
                }
            }
        }
        memo.depend(at, 1);
        let Some(parent) = node.parent else { break };
        (child, at) = (Some(at), parent);
    }
    false
}

// Match the `submit` type of buttons in dom.js.
pub(super) fn submits(node: &crate::dom::Node) -> bool {
    let kind = node.attrs.get("type").map_or("", String::as_str);
    match node.tag.as_str() {
        "input" => kind.eq_ignore_ascii_case("submit") || kind.eq_ignore_ascii_case("image"),
        "button" => {
            kind.eq_ignore_ascii_case("submit")
                || !kind.eq_ignore_ascii_case("reset")
                    && !kind.eq_ignore_ascii_case("button")
                    && !node.attrs.contains_key("commandfor")
        }
        _ => false,
    }
}

// Elements that can be invalid: form controls and form-associated custom elements.
fn submittable(node: &crate::dom::Node) -> bool {
    matches!(
        node.tag.as_str(),
        "input" | "button" | "select" | "textarea"
    ) || node.state & crate::dom::FORM_ASSOCIATED != 0
}

// Match `candidate` in dom.js, which also sets the validity bit.
fn validated(doc: &Document, id: usize, memo: &mut SelectorCache) -> bool {
    memo.depend(id, 0);
    memo.depend(id, 1);
    memo.depend(id, 5);
    let node = &doc.nodes[id];
    let candidate = match node.tag.as_str() {
        "input" => !["hidden", "reset", "button", "image"]
            .iter()
            .any(|kind| node.input_type().eq_ignore_ascii_case(kind)),
        "button" => submits(node),
        _ => submittable(node),
    };
    candidate
        && !node.attrs.contains_key("readonly")
        && node.parent.is_none_or(|parent| {
            memo.depend(parent, 4);
            !doc.nodes[parent].tag.starts_with("datalist")
        })
        && !disabled(doc, id, memo)
}

// Match `formOf` in dom.js.
fn form_of(doc: &Document, id: usize, memo: &mut SelectorCache) -> Option<usize> {
    memo.depend(id, 0);
    if doc.nodes[id].attrs.contains_key("form") {
        memo.depend(id, 2);
        return memo.index(doc, id).owner(id);
    }
    let mut at = id;
    loop {
        memo.depend(at, 1);
        at = doc.nodes[at].parent?;
        memo.depend(at, 4);
        if doc.nodes[at].tag == "form" {
            return Some(at);
        }
    }
}

// The first control in the form or the fieldset `id` that passes `test`. The form's controls outside of it count
// by tree order only when `ordered`.
fn first_control(
    doc: &Document,
    id: usize,
    memo: &mut SelectorCache,
    ordered: bool,
    test: impl Fn(usize, &mut SelectorCache) -> bool,
) -> Option<usize> {
    let form = doc.nodes[id].tag == "form";
    let mut first = None;
    if form {
        memo.depend(id, 2);
        let outside = memo.index(doc, id).outside(id).to_vec();
        for at in outside {
            memo.depend(at, 0);
            if submittable(&doc.nodes[at])
                && test(at, memo)
                && first.is_none_or(|first| precedes(doc, at, first, memo))
            {
                first = Some(at);
                if !ordered {
                    return first;
                }
            }
        }
    }
    let mut pending = vec![id];
    while let Some(at) = pending.pop() {
        let node = &doc.nodes[at];
        if node.tag.starts_with('#') {
            continue;
        }
        if at != id && submittable(node) {
            memo.depend(at, 0);
            if !(form && node.attrs.contains_key("form")) && test(at, memo) {
                return match first {
                    Some(first) if precedes(doc, first, at, memo) => Some(first),
                    _ => Some(at),
                };
            }
        }
        memo.depend(at, 2);
        if node.tag.contains('-') {
            memo.depend(at, 5);
        }
        if node.tag != "template" && (at == id || !form || node.tag != "form") {
            pending.extend(node.children.iter().rev());
        }
    }
    first
}

fn precedes(doc: &Document, a: usize, b: usize, memo: &mut SelectorCache) -> bool {
    controls::precedes(doc, a, b, |id, children| {
        memo.depend(id, if children { 2 } else { 1 })
    })
}

impl SelectorCache {
    pub(crate) fn matches(
        &mut self,
        selector: &Selector,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
    ) -> bool {
        matches_with(selector, doc, id, scope, self, true, true)
    }

    // Matches without the kept results, which an update has not checked yet.
    pub(crate) fn matches_fresh(&mut self, selector: &Selector, doc: &Document, id: usize) -> bool {
        matches_with(selector, doc, id, None, self, false, true)
    }

    // The controls of the page, or those in the tree of `id` for a match outside of a style pass.
    fn index(&mut self, doc: &Document, id: usize) -> &Controls {
        if self.nodes.is_empty() && !self.controls.whole {
            self.controls = Controls::of(doc, id);
        }
        &self.controls
    }

    // Records the classes and attributes that the selector reads.
    pub(crate) fn note_selector(&mut self, selector: &Selector, subject: bool) {
        note_attributes(selector, &mut self.attributes, subject);
    }
}

// Whether node `id` matches the selector list in `source`.
pub fn matches(doc: &Document, id: usize, source: &str) -> bool {
    Selector::parse(source, None)
        .iter()
        .any(|selector| SelectorCache::default().matches(selector, doc, id, None))
}

fn note_attributes(selector: &Selector, names: &mut Named, subject: bool) {
    if let Some((_, previous)) = &selector.previous {
        note_attributes(previous, names, false);
    }
    for simple in &selector.simple {
        match simple {
            SimpleSelector::Class(class) if !subject => {
                let class = rustc_hash::FxBuildHasher.hash_one(class.as_str()) as u32;
                names.generation += u32::from(names.classes.insert(class));
            }
            SimpleSelector::Attribute(name, operator, ..) => {
                let name = name.to_ascii_lowercase();
                let valued = !operator.is_empty() && names.valued.insert(name.clone());
                names.generation += u32::from(valued) + u32::from(names.names.insert(name));
            }
            SimpleSelector::Pseudo(_, _, parsed, ..) => {
                for selector in parsed.iter() {
                    note_attributes(selector, names, false);
                }
            }
            _ => {}
        }
    }
}

// Whether the node has the tag, id and classes of the compound.
fn plain(selector: &Selector, node: &crate::dom::Node) -> bool {
    selector.simple.iter().all(|simple| match simple {
        SimpleSelector::Tag(tag) => node.tag == *tag || node.tag.eq_ignore_ascii_case(tag),
        SimpleSelector::Id(id) => node.attrs.get("id") == Some(id),
        SimpleSelector::Class(class) => node
            .attrs
            .get("class")
            .is_some_and(|classes| classes.split_ascii_whitespace().any(|c| c == class)),
        _ => true,
    })
}

// The kinds that a match of this `:has()` argument can read from a node, besides its place in the tree.
pub(super) fn reads(selector: &Selector) -> u16 {
    let above = selector.previous.as_ref();
    let above = above.map_or(0, |(_, previous)| reads(previous));
    selector.simple.iter().fold(above, |all, simple| {
        all | match simple {
            SimpleSelector::Class(_) => 1 << 8,
            SimpleSelector::Universal => 0,
            SimpleSelector::Pseudo(name, _, parsed, ..)
                if matches!(name.as_str(), "scope" | "is" | "where" | "not") =>
            {
                parsed.iter().fold(0, |all, selector| all | reads(selector))
            }
            _ => 1 | 1 << 3 | 1 << 5,
        }
    })
}

/// Whether `dir=auto` of an element is right-to-left, from the first letter of its text.
fn auto_rtl(doc: &Document, id: usize, memo: &mut SelectorCache) -> bool {
    memo.depend(id, 0);
    memo.depend(id, 2);
    memo.depend(id, 3);
    let node = &doc.nodes[id];
    if let Some(value) = node.current_value() {
        return renderer::right_to_left(value) == Some(true);
    }
    let mut pending: Vec<usize> = node.children.iter().rev().copied().collect();
    while let Some(child) = pending.pop() {
        memo.depend(child, 3);
        let node = &doc.nodes[child];
        if node.tag == "#text" {
            if let Some(rtl) = renderer::right_to_left(&node.text) {
                return rtl;
            }
            continue;
        }
        memo.depend(child, 0);
        memo.depend(child, 2);
        if !matches!(node.tag.as_str(), "bdi" | "script" | "style" | "textarea")
            && !node.attrs.contains_key("dir")
        {
            pending.extend(node.children.iter().rev());
        }
    }
    false
}

// Whether a match of this `:has()` argument reads only nodes below the anchor, or below its parent when
// the argument starts at a sibling, and no kind that `update` leaves out.
fn local(selector: &Selector, anchor: &crate::dom::Node) -> bool {
    selector.simple.iter().all(|simple| match simple {
        SimpleSelector::Attribute(name, ..) => !name.eq_ignore_ascii_case("style"),
        SimpleSelector::Pseudo(name, _, parsed, ..) => {
            // These read ancestors. A checked option reads its select, which is above an optgroup.
            !matches!(
                name.as_str(),
                "lang"
                    | "dir"
                    | "read-only"
                    | "read-write"
                    | "host"
                    | "disabled"
                    | "enabled"
                    | "valid"
                    | "invalid"
                    | "user-valid"
                    | "user-invalid"
                    | "indeterminate"
                    | "in-range"
                    | "out-of-range"
                    | "default"
            ) && !(name == "checked" && matches!(anchor.tag.as_str(), "optgroup" | "option"))
                && parsed
                    .iter()
                    .all(|selector| selector.previous.is_none() && local(selector, anchor))
        }
        SimpleSelector::Parent(..) => false,
        _ => true,
    }) && selector
        .previous
        .as_ref()
        .is_none_or(|(_, previous)| local(previous, anchor))
}

// Whether a relative selector of a `:has()` matches below or after `id`, and whether the selectors read
// its siblings. A local scan records no dependencies of the nodes that fail.
pub(super) fn has(
    parsed: &[Selector],
    doc: &Document,
    id: usize,
    memo: &mut SelectorCache,
    local: bool,
) -> (bool, bool) {
    let node = &doc.nodes[id];
    let (mut children, mut siblings, mut depth) = (false, 0usize, 0usize);
    for selector in parsed {
        let (mut levels, mut steps, mut current) = (0usize, 0usize, selector);
        while let Some((combinator, previous)) = &current.previous {
            (levels, steps) = match combinator {
                '+' => (levels, steps.saturating_add(1)),
                '~' => (levels, usize::MAX),
                '>' => (levels.saturating_add(1), 0),
                _ => (usize::MAX, 0),
            };
            if previous.previous.is_none() {
                children |= matches!(combinator, '>' | ' ');
                siblings = siblings.max(steps);
            }
            current = previous;
        }
        depth = depth.max(levels);
    }
    let mut pending = Vec::new();
    if children {
        memo.depend(id, 2);
        pending.extend(node.children.iter().map(|&child| (child, 1)));
    }
    let parent = node.parent.filter(|_| siblings > 0);
    if let Some(parent) = parent {
        memo.depend(id, 1);
        memo.depend(parent, 2);
        let positions = memo.positions(doc, parent);
        if let Some(place) = positions.0.get(&id) {
            let after = positions.1[place[0] + 1..].iter().take(siblings);
            pending.extend(after.map(|&sibling| (sibling, 0)));
        }
    }
    while let Some((child, level)) = pending.pop() {
        let below = &doc.nodes[child];
        if below.tag.starts_with('#') {
            continue;
        }
        if parsed.iter().any(|selector| {
            (!local || plain(selector, below))
                && matches_nested(selector, doc, child, Some(id), memo)
        }) {
            return (true, parent.is_some());
        }
        if level < depth {
            if !local {
                memo.depend(child, 2);
            }
            let children = below.children.iter();
            pending.extend(children.map(|&grandchild| (grandchild, level + 1)));
        }
    }
    (false, parent.is_some())
}

fn matches_nested(
    selector: &Selector,
    doc: &Document,
    id: usize,
    scope: Option<usize>,
    memo: &mut SelectorCache,
) -> bool {
    matches_with(selector, doc, id, scope, memo, true, false)
}

fn matches_with(
    selector: &Selector,
    doc: &Document,
    id: usize,
    scope: Option<usize>,
    memo: &mut SelectorCache,
    store: bool,
    subject: bool,
) -> bool {
    let key = match_key(selector as *const Selector as usize, id, scope, false);
    let compound = !store
        || selector.previous.is_none()
            && selector.simple.iter().all(|simple| match simple {
                SimpleSelector::Pseudo(_, _, parsed, ..) => parsed.is_empty(),
                SimpleSelector::Parent(..) => false,
                _ => true,
            });
    let start = memo.dependencies.len();
    let classes = selector
        .simple
        .iter()
        .any(|simple| matches!(simple, SimpleSelector::Class(_)));
    if !classes
        || selector
            .simple
            .iter()
            .any(|simple| !matches!(simple, SimpleSelector::Class(_) | SimpleSelector::Universal))
    {
        memo.depend(id, 0);
    }
    if classes {
        memo.depend(id, if subject { 7 } else { 8 });
    }
    if selector.simple.iter().any(|simple| {
        matches!(simple, SimpleSelector::Attribute(name, ..) if name.eq_ignore_ascii_case("style"))
    }) {
        memo.depend(id, 6);
    }
    let node = &doc.nodes[id];
    // Most candidates of a scan fail here, so it runs before the lookup and stores no result.
    if !plain(selector, node) {
        return false;
    }
    if !compound
        && let Some(&(at, len)) = memo.values.get(&key)
        && let found = &memo.found[at as usize..][..(len & !MATCHED) as usize]
        && memo.valid(found)
    {
        memo.dependencies.truncate(start);
        memo.dependencies.extend_from_slice(found);
        return len & MATCHED != 0;
    }
    let check = |s: &SimpleSelector, memo: &mut SelectorCache| match s {
        SimpleSelector::Tag(_) | SimpleSelector::Id(_) | SimpleSelector::Class(_) => true,
        SimpleSelector::Universal => true,
        SimpleSelector::Root => node.tag == "html",
        SimpleSelector::Attribute(name, operator, value, flag) => node
            .attrs
            .iter()
            .find(|(key, _)| *key == name || key.eq_ignore_ascii_case(name))
            .is_some_and(|(_, actual)| {
                let insensitive =
                    *flag == 1 || *flag == 2 && !node.tag.bytes().any(|c| c.is_ascii_uppercase());
                let equal = |part: Option<&str>| {
                    part.is_some_and(|part| {
                        if insensitive {
                            part.eq_ignore_ascii_case(value)
                        } else {
                            part == value
                        }
                    })
                };
                let length = value.len();
                match operator.as_str() {
                    "" => true,
                    "=" => equal(Some(actual)),
                    "~=" => actual
                        .split_ascii_whitespace()
                        .any(|word| equal(Some(word))),
                    "|=" => {
                        equal(actual.get(..length))
                            && actual.as_bytes().get(length).is_none_or(|&c| c == b'-')
                    }
                    _ if value.is_empty() => false,
                    "^=" => equal(actual.get(..length)),
                    "$=" => equal(
                        actual
                            .len()
                            .checked_sub(length)
                            .and_then(|start| actual.get(start..)),
                    ),
                    "*=" if insensitive => actual
                        .as_bytes()
                        .windows(length)
                        .any(|part| part.eq_ignore_ascii_case(value.as_bytes())),
                    "*=" => actual.contains(value.as_str()),
                    _ => false,
                }
            }),
        SimpleSelector::Pseudo(name, argument, parsed, [a, b], _) => match name.as_str() {
            "scope" => scope.map_or(node.tag == "html", |scope| {
                scope == id || node.parent == Some(scope) && doc.nodes[scope].tag == "#document"
            }),
            "has" => {
                let local = memo.nodes.get(id).is_some()
                    && parsed.iter().all(|selector| local(selector, node));
                if !local {
                    return has(parsed, doc, id, memo, false).0;
                }
                let known = memo.within.get(&id).and_then(|results| {
                    results.iter().find(|result| Rc::ptr_eq(&result.0, parsed))
                });
                let (found, beside) = match known {
                    Some(result) => (result.1, result.2),
                    None => {
                        let start = memo.dependencies.len();
                        let (found, beside) = has(parsed, doc, id, memo, true);
                        memo.dependencies.truncate(start);
                        let results = memo.within.entry(id).or_default();
                        results.push((parsed.clone(), found, beside));
                        match node.parent.filter(|_| beside) {
                            Some(parent) => memo.watched[parent] |= BESIDE,
                            None => memo.watched[id] |= BELOW,
                        }
                        (found, beside)
                    }
                };
                if beside {
                    memo.depend(id, 1);
                }
                memo.depend(id, WITHIN);
                found
            }
            "empty" => {
                memo.depend(id, 2);
                memo.depend(id, 3);
                for &child in &node.children {
                    memo.depend(child, 3);
                }
                node.text.is_empty()
                    && node.children.iter().all(|&id| {
                        doc.nodes[id].tag == "#comment"
                            || doc.nodes[id].tag == "#text" && doc.nodes[id].text.is_empty()
                    })
            }
            "hover" | "active" | "focus" | "focus-visible" | "focus-within" => {
                memo.depend(id, 5);
                node.state
                    & match name.as_str() {
                        "hover" => HOVER,
                        "active" => ACTIVE,
                        "focus-within" => FOCUS_WITHIN,
                        "focus-visible" => FOCUS_VISIBLE,
                        _ => FOCUS,
                    }
                    != 0
            }
            "in-range" | "out-of-range" => {
                memo.depend(id, 0);
                memo.depend(id, 3);
                let value = node.current_value().unwrap_or("");
                let bound = |name| node.attrs.get(name).map(String::as_str);
                let numeric = node.input_type().eq_ignore_ascii_case("number");
                // Dates and times in their valid forms compare as strings.
                let beyond = |limit: Option<&str>, below: bool| {
                    limit.is_some_and(|limit| {
                        let order = match (value.parse::<f64>(), limit.parse::<f64>()) {
                            (Ok(value), Ok(limit)) if numeric => value.partial_cmp(&limit),
                            _ if numeric || value.is_empty() || limit.len() != value.len() => None,
                            _ => Some(value.cmp(limit)),
                        };
                        order
                            == Some(if below {
                                std::cmp::Ordering::Less
                            } else {
                                std::cmp::Ordering::Greater
                            })
                    })
                };
                let out = !node.input_type().eq_ignore_ascii_case("range")
                    && (beyond(bound("min"), true) || beyond(bound("max"), false));
                ranged(node)
                    && !node.attrs.contains_key("readonly")
                    && !disabled(doc, id, memo)
                    && out == (name == "out-of-range")
            }
            "target" => {
                memo.depend(id, 5);
                doc.target == Some(id)
            }
            "link" | "any-link" | "-webkit-any-link" => {
                matches!(node.tag.as_str(), "a" | "area") && node.attrs.contains_key("href")
            }
            "defined" => {
                memo.depend(id, 5);
                node.state & crate::dom::DEFINED != 0
                    || !node.tag.contains('-')
                    || matches!(
                        node.tag.as_str(),
                        "annotation-xml"
                            | "color-profile"
                            | "font-face"
                            | "font-face-src"
                            | "font-face-uri"
                            | "font-face-format"
                            | "font-face-name"
                            | "missing-glyph"
                    )
            }
            "open" => {
                matches!(node.tag.as_str(), "details" | "dialog") && node.attrs.contains_key("open")
            }
            "indeterminate" => {
                memo.depend(id, 5);
                node.tag == "progress" && !node.attrs.contains_key("value")
                    || node.tag == "input"
                        && node.state & crate::dom::INDETERMINATE != 0
                        && node.input_type().eq_ignore_ascii_case("checkbox")
                    || radio(node) && {
                        memo.depend(id, 0);
                        memo.depend(id, 2);
                        !node.checked() && !memo.index(doc, id).checked(id)
                    }
            }
            "default" => {
                let checkable = node.tag == "input"
                    && ["checkbox", "radio"]
                        .iter()
                        .any(|kind| node.input_type().eq_ignore_ascii_case(kind));
                if checkable || node.tag == "option" {
                    node.attrs
                        .contains_key(if checkable { "checked" } else { "selected" })
                } else {
                    submits(node)
                        && form_of(doc, id, memo).is_some_and(|form| {
                            memo.depend(form, 2);
                            // Kept like the positions of `:nth-child(An+B of S)`.
                            let key = (0, form, None);
                            let first = match memo.nth.get(&key) {
                                Some(first) => first.clone(),
                                None => {
                                    let start = memo.dependencies.len();
                                    let first = first_control(doc, form, memo, true, |at, _| {
                                        submits(&doc.nodes[at])
                                    });
                                    let mut dependencies = memo.dependencies.split_off(start);
                                    dependencies.sort_unstable();
                                    dependencies.dedup();
                                    let first = first.map(|at| (at, 0)).into_iter().collect();
                                    let first = Rc::new((first, dependencies));
                                    memo.nth.insert(key, first.clone());
                                    first
                                }
                            };
                            first.0.contains_key(&id)
                        })
                }
            }
            "checked" => {
                memo.depend(id, 0);
                memo.depend(id, 5);
                match node.tag.as_str() {
                    "input" => {
                        node.checked()
                            && ["checkbox", "radio"]
                                .iter()
                                .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
                    }
                    "option" if node.state & crate::dom::DIRTY != 0 || node.checked() => {
                        node.checked()
                    }
                    "option" => {
                        let mut select = node.parent.unwrap_or_default();
                        memo.depend(select, 1);
                        if doc.nodes[select].tag == "optgroup" {
                            select = doc.nodes[select].parent.unwrap_or_default();
                        }
                        memo.depend(select, 0);
                        memo.depend(select, 2);
                        let parent = &doc.nodes[select];
                        let mut options = Vec::new();
                        for &child in &parent.children {
                            if doc.nodes[child].tag == "optgroup" {
                                memo.depend(child, 2);
                                options.extend(&doc.nodes[child].children);
                            } else {
                                options.push(child);
                            }
                        }
                        options.retain(|&option| doc.nodes[option].tag == "option");
                        for &option in &options {
                            memo.depend(option, 0);
                            memo.depend(option, 5);
                        }
                        parent.tag == "select"
                            && !parent.attrs.contains_key("multiple")
                            && parent.attrs.get("size").is_none_or(|size| {
                                size.trim().parse::<u32>().is_ok_and(|size| size <= 1)
                            })
                            && options.iter().all(|&option| !doc.nodes[option].checked())
                            && options
                                .iter()
                                .find(|&&option| !doc.nodes[option].attrs.contains_key("disabled"))
                                == Some(&id)
                    }
                    _ => false,
                }
            }
            // The root holds the types of the active view transition in `:vt`.
            "active-view-transition" | "active-view-transition-type" => {
                memo.depend(id, 0);
                node.attrs.get(":vt").is_some_and(|types| {
                    name.len() == 22
                        || argument
                            .split(',')
                            .any(|wanted| types.split(' ').any(|kind| kind == wanted.trim()))
                })
            }
            "state" => {
                memo.depend(id, 3);
                node.tag.contains('-')
                    && node
                        .value
                        .as_ref()
                        .is_some_and(|states| states.split('\0').any(|state| state == argument))
            }
            "modal" | "popover-open" | "fullscreen" | "-webkit-full-screen" => {
                memo.depend(id, 5);
                node.state
                    & match name.as_str() {
                        "modal" => crate::dom::MODAL,
                        "popover-open" => crate::dom::POPOVER,
                        _ => crate::dom::FULLSCREEN,
                    }
                    != 0
            }
            "-internal-dialog-in-top-layer" => {
                memo.depend(id, 5);
                node.tag == "dialog"
                    && node.state & (crate::dom::MODAL | crate::dom::MODAL << 5) != 0
            }
            "disabled" | "enabled" => {
                (matches!(
                    node.tag.as_str(),
                    "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
                ) || node.tag.contains('-') && {
                    memo.depend(id, 5);
                    node.state & crate::dom::FORM_ASSOCIATED != 0
                }) && disabled(doc, id, memo) == (name == "disabled")
            }
            "valid" | "invalid" if matches!(node.tag.as_str(), "form" | "fieldset") => {
                let invalid = first_control(doc, id, memo, false, |at, memo| {
                    doc.nodes[at].state & crate::dom::INVALID != 0 && validated(doc, at, memo) || {
                        memo.depend(at, 5);
                        false
                    }
                });
                invalid.is_some() == (name == "invalid")
            }
            "valid" | "invalid" | "user-valid" | "user-invalid" => {
                memo.depend(id, 5);
                let invalid = node.state & crate::dom::INVALID != 0;
                (!name.starts_with("user") || node.state & crate::dom::USER != 0)
                    && if node.state & crate::dom::FORM_ASSOCIATED != 0 {
                        // As in Chrome, a custom element that is not validated is valid.
                        (invalid && validated(doc, id, memo)) == (name == "invalid")
                    } else {
                        invalid == name.ends_with("invalid") && validated(doc, id, memo)
                    }
            }
            "placeholder-shown" | "required" | "optional" => {
                memo.depend(id, 2);
                memo.depend(id, 3);
                memo.depend(id, 5);
                for &child in &node.children {
                    memo.depend(child, 3);
                }
                let required = node.attrs.contains_key("required");
                let empty = if ["checkbox", "radio"]
                    .iter()
                    .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
                {
                    !node.checked()
                } else {
                    node.current_value().map_or(
                        node.text.is_empty()
                            && node.children.iter().all(|&child| {
                                doc.nodes[child].tag == "#text" && doc.nodes[child].text.is_empty()
                            }),
                        str::is_empty,
                    )
                };
                matches!(node.tag.as_str(), "input" | "select" | "textarea")
                    && match name.as_str() {
                        "placeholder-shown" => {
                            node.tag != "select" && node.attrs.contains_key("placeholder") && empty
                        }
                        "required" => required,
                        _ => !required,
                    }
            }
            "read-write" | "read-only" if matches!(node.tag.as_str(), "input" | "textarea") => {
                let writable = !node.attrs.contains_key("readonly")
                    && !disabled(doc, id, memo)
                    && (node.tag == "textarea"
                        || ![
                            "hidden", "checkbox", "radio", "file", "submit", "image", "reset",
                            "button", "color", "range",
                        ]
                        .iter()
                        .any(|other| node.input_type().eq_ignore_ascii_case(other)));
                writable == (name == "read-write")
            }
            "read-write" | "read-only" | "lang" | "dir" => {
                let attribute = match name.as_str() {
                    "lang" | "dir" => name.as_str(),
                    _ => "contenteditable",
                };
                let mut value = None;
                let mut found = None;
                let mut current = Some(id);
                while value.is_none()
                    && let Some(ancestor) = current
                {
                    memo.depend(ancestor, 0);
                    memo.depend(ancestor, 1);
                    value =
                        doc.nodes[ancestor]
                            .attrs
                            .get(attribute)
                            .filter(|value| match attribute {
                                "lang" => true,
                                "dir" => ["ltr", "rtl", "auto"]
                                    .iter()
                                    .any(|v| value.eq_ignore_ascii_case(v)),
                                _ => crate::dom::editable_value(value).is_some(),
                            });
                    found = Some(ancestor);
                    // Without a direction, a `<bdi>` is `dir=auto`.
                    if attribute == "dir" && value.is_none() && doc.nodes[ancestor].tag == "bdi" {
                        break;
                    }
                    current = doc.nodes[ancestor].parent;
                }
                match name.as_str() {
                    "lang" => value.is_some_and(|value| {
                        !value.is_empty()
                            && argument.split(',').any(|range| {
                                let range = range.trim().trim_matches(['"', '\'']);
                                range == "*"
                                    || value.as_bytes().get(..range.len()).is_some_and(|prefix| {
                                        prefix.eq_ignore_ascii_case(range.as_bytes())
                                    }) && value
                                        .as_bytes()
                                        .get(range.len())
                                        .is_none_or(|&c| c == b'-')
                            })
                    }),
                    "dir" => {
                        let rtl = match value {
                            Some(value) if !value.eq_ignore_ascii_case("auto") => {
                                value.eq_ignore_ascii_case("rtl")
                            }
                            value => found.is_some_and(|found| {
                                (value.is_some() || doc.nodes[found].tag == "bdi")
                                    && auto_rtl(doc, found, memo)
                            }),
                        };
                        let wanted = argument.trim();
                        wanted.eq_ignore_ascii_case("rtl") && rtl
                            || wanted.eq_ignore_ascii_case("ltr") && !rtl
                    }
                    _ => {
                        (value.and_then(|value| crate::dom::editable_value(value)) == Some(true))
                            == (name == "read-write")
                    }
                }
            }
            "not" => !parsed
                .iter()
                .any(|s| matches_nested(s, doc, id, scope, memo)),
            "is" | "where" => parsed
                .iter()
                .any(|s| matches_nested(s, doc, id, scope, memo)),
            "first-child" | "last-child" | "only-child" | "nth-child" | "nth-last-child"
            | "first-of-type" | "last-of-type" | "only-of-type" | "nth-of-type"
            | "nth-last-of-type" => {
                memo.depend(id, 1);
                node.parent.is_some_and(|parent| {
                    let siblings = &doc.nodes[parent].children;
                    let (index, count) = if parsed.is_empty() {
                        match name.as_str() {
                            "first-child" | "last-child" | "only-child" => {
                                memo.placed(doc, id, EDGE)
                            }
                            "nth-child" => memo.placed(doc, id, INDEX),
                            _ => memo.depend(parent, 2),
                        }
                        let positions = memo.positions(doc, parent);
                        let Some(&[index, count, of_type, type_count]) = positions.0.get(&id)
                        else {
                            return false;
                        };
                        if name.ends_with("of-type") {
                            (of_type, type_count)
                        } else {
                            (index, count)
                        }
                    } else {
                        memo.depend(parent, 2);
                        if !parsed
                            .iter()
                            .any(|s| matches_nested(s, doc, id, scope, memo))
                        {
                            return false;
                        }
                        let key = (parsed.as_ptr() as usize, parent, scope);
                        let positions = match memo.nth.get(&key) {
                            Some(positions) => positions.clone(),
                            None => {
                                let start = memo.dependencies.len();
                                memo.depend(parent, 2);
                                let mut indices = rustc_hash::FxHashMap::default();
                                for &child in siblings {
                                    if !doc.nodes[child].tag.starts_with('#')
                                        && parsed
                                            .iter()
                                            .any(|s| matches_nested(s, doc, child, scope, memo))
                                    {
                                        indices.insert(child, indices.len());
                                    }
                                }
                                let mut dependencies = memo.dependencies.split_off(start);
                                dependencies.sort_unstable();
                                dependencies.dedup();
                                let positions = Rc::new((indices, dependencies));
                                memo.nth.insert(key, positions.clone());
                                positions
                            }
                        };
                        let Some(&index) = positions.0.get(&id) else {
                            return false;
                        };
                        (index, positions.0.len())
                    };
                    match name.as_str() {
                        "first-child" | "first-of-type" => index == 0,
                        "last-child" | "last-of-type" => index + 1 == count,
                        "only-child" | "only-of-type" => count == 1,
                        _ => {
                            let index = if name.starts_with("nth-last") {
                                count - 1 - index
                            } else {
                                index
                            };
                            let (a, n) = (*a as i64, index as i64 + 1 - *b as i64);
                            if a == 0 {
                                n == 0
                            } else {
                                n % a == 0 && n / a >= 0
                            }
                        }
                    }
                })
            }
            _ => false,
        },
        SimpleSelector::Element(..) | SimpleSelector::NoNamespace => false,
        SimpleSelector::Parent(parent, _) => parent
            .iter()
            .any(|s| matches_nested(s, doc, id, scope, memo)),
    };
    // Place checks read the siblings of the node. They run after the other selectors of the chain, so a failed
    // chain keeps them out of the dependencies, and its nodes can share their styles.
    let late = selector.previous.is_some() && selector.simple.iter().any(positional);
    let result = (|| {
        let host = node
            .tag
            .starts_with('#')
            .then(|| memo.hosts.get(&id).copied())
            .flatten();
        if let Some(host) = host {
            let mut hosted = false;
            for simple in &selector.simple {
                let matched = match simple {
                    SimpleSelector::Universal => continue,
                    SimpleSelector::Pseudo(name, _, parsed, ..) if name == "host" => {
                        parsed.is_empty()
                            || parsed
                                .iter()
                                .any(|s| matches_nested(s, doc, host, scope, memo))
                    }
                    SimpleSelector::Pseudo(name, _, parsed, ..)
                        if matches!(name.as_str(), "is" | "where") =>
                    {
                        parsed
                            .iter()
                            .any(|s| matches_nested(s, doc, id, scope, memo))
                    }
                    SimpleSelector::Parent(parent, _) => parent
                        .iter()
                        .any(|s| matches_nested(s, doc, id, scope, memo)),
                    _ => false,
                };
                if !matched {
                    return false;
                }
                hosted = true;
            }
            if !hosted {
                return false;
            }
        } else if node.tag.starts_with('#')
            || !selector
                .simple
                .iter()
                .filter(|s| !late || !positional(s))
                .all(|s| check(s, memo))
        {
            return false;
        }
        let Some((combinator, previous)) = &selector.previous else {
            return true;
        };
        memo.depend(id, 1);
        let Some(parent) = node.parent else {
            return false;
        };
        let chained = match combinator {
            '>' => matches_nested(previous, doc, parent, scope, memo),
            '+' => {
                memo.placed(doc, id, PREVIOUS);
                let positions = memo.positions(doc, parent);
                let index = positions
                    .0
                    .get(&id)
                    .and_then(|place| place[0].checked_sub(1));
                index.is_some_and(|index| {
                    matches_nested(previous, doc, positions.1[index], scope, memo)
                })
            }
            '~' => {
                memo.depend(parent, 2);
                let positions = memo.positions(doc, parent);
                // Keep the first sibling that matches, like the positions of `:nth-child(An+B of S)`:
                // a scan from each node costs O(N²) time and dependencies.
                let key = (&**previous as *const Selector as usize, parent, scope);
                let first = match memo.nth.get(&key) {
                    Some(first) => first.clone(),
                    None => {
                        let start = memo.dependencies.len();
                        memo.depend(parent, 2);
                        let first = positions.1.iter().enumerate().find(|&(_, &sibling)| {
                            matches_nested(previous, doc, sibling, scope, memo)
                        });
                        let first = first.map(|(index, &sibling)| (sibling, index));
                        let mut dependencies = memo.dependencies.split_off(start);
                        dependencies.sort_unstable();
                        dependencies.dedup();
                        let first = Rc::new((first.into_iter().collect(), dependencies));
                        memo.nth.insert(key, first.clone());
                        first
                    }
                };
                let index = positions.0.get(&id).map(|place| place[0]);
                index.is_some_and(|index| first.0.values().any(|&first| first < index))
            }
            _ => {
                let mut ancestor = Some(parent);
                let mut visited = Vec::new();
                let mut matched = false;
                while let Some(id) = ancestor {
                    // Memoize only reads from the ancestor upward, or sibling lookups inherit each
                    // other's branch dependencies.
                    let from = memo.dependencies.len();
                    memo.depend(id, 1);
                    let key = match_key(&**previous as *const Selector as usize, id, scope, true);
                    if let Some(&(at, len)) = memo.values.get(&key)
                        && let found = &memo.found[at as usize..][..(len & !MATCHED) as usize]
                        && memo.valid(found)
                    {
                        matched = len & MATCHED != 0;
                        memo.dependencies.extend_from_slice(found);
                        break;
                    }
                    visited.push((key, from));
                    if matches_nested(previous, doc, id, scope, memo) {
                        matched = true;
                        break;
                    }
                    ancestor = doc.nodes[id].parent;
                }
                for (key, from) in visited.into_iter().rev() {
                    memo.store(key, matched, from);
                }
                matched
            }
        };
        chained
            && (!late
                || selector
                    .simple
                    .iter()
                    .filter(|s| positional(s))
                    .all(|s| check(s, memo)))
    })();
    if compound {
        return result;
    }
    memo.store(key, result, start);
    result
}
