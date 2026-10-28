use bbtree::unsync::BBTreeMap;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, rc::Rc};

pub(crate) type SelectorList = Rc<Vec<Selector>>;

pub(crate) const FOCUS: u8 = 1;
pub(crate) const FOCUS_WITHIN: u8 = 2;
pub(crate) const HOVER: u8 = 4;
pub(crate) const ACTIVE: u8 = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Selector {
    simple: Vec<SimpleSelector>,
    previous: Option<(char, Box<Selector>)>,
    pub specificity: [usize; 3],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
enum SimpleSelector {
    Tag(String),
    Id(String),
    Class(String),
    Universal,
    Root,
    Attribute(String, String, String, bool),
    Pseudo(String, String, #[serde(skip)] Vec<Selector>, [i32; 2]),
    Element(String, String),
    Parent(#[serde(skip)] SelectorList, usize),
}

fn split(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = source;
    loop {
        let end = selector_boundary(rest, |c| c == ',');
        parts.push(&rest[..end]);
        if end == rest.len() {
            return parts;
        }
        rest = &rest[end + 1..];
    }
}

fn unescape(source: &str, stop: impl Fn(char) -> bool) -> (String, usize) {
    let mut value = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c != '\\' {
            if stop(c) {
                return (value, index);
            }
            value.push(c);
            continue;
        }
        let (mut code, mut digits) = (0, 0);
        while digits < 6
            && let Some(digit) = chars.peek().and_then(|(_, c)| c.to_digit(16))
        {
            code = code * 16 + digit;
            digits += 1;
            chars.next();
        }
        if digits > 0 {
            chars.next_if(|(_, c)| c.is_ascii_whitespace());
            value.push(
                char::from_u32(code)
                    .filter(|&c| c != '\0')
                    .unwrap_or('\u{fffd}'),
            );
        } else if let Some((_, c)) = chars.next()
            && c != '\n'
        {
            value.push(c);
        }
    }
    (value, source.len())
}

fn identifier(source: &str) -> Option<(String, usize)> {
    let (name, length) = unescape(source, |c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii())
    });
    let raw = source.as_bytes();
    let invalid = length == 0
        || raw[0].is_ascii_digit()
        || raw[0] == b'-' && (length == 1 || raw[1].is_ascii_digit());
    (!invalid).then_some((name, length))
}

fn selector_boundary(source: &str, separator: impl Fn(char) -> bool) -> usize {
    let mut depth = 0;
    let mut quote = None;
    let mut escape = None;
    for (index, c) in source.char_indices() {
        if let Some(digits) = escape {
            if c.is_ascii_hexdigit() && digits < 6 {
                escape = Some(digits + 1);
                continue;
            }
            escape = None;
            if digits == 0 || c.is_ascii_whitespace() {
                continue;
            }
        }
        if c == '\\' {
            escape = Some(0);
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, '\'' | '"') {
            quote = Some(c);
            continue;
        }
        if depth == 0 && separator(c) {
            return index;
        }
        if matches!(c, '[' | '(') {
            depth += 1;
        } else if matches!(c, ']' | ')') {
            depth -= 1;
        }
    }
    source.len()
}

#[derive(Default)]
pub(crate) struct SelectorCache {
    nodes: Vec<(Node, [u64; 6])>,
    values: HashMap<(usize, usize, Option<usize>, bool), (bool, Rc<[(usize, u64)]>)>,
    pub(crate) dependencies: Vec<(usize, u64)>,
    pub(crate) epoch: u64,
    changes: std::collections::VecDeque<(u64, Vec<usize>)>,
}

impl SelectorCache {
    pub fn update(&mut self, doc: &Document, order: &[usize]) {
        if self.values.len() > 131072 {
            self.values.clear();
        }
        self.nodes
            .resize_with(doc.nodes.len(), || (Node::default(), [0; 6]));
        let mut changes = Vec::new();
        for &id in order {
            let node = &doc.nodes[id];
            let (old, revisions) = &mut self.nodes[id];
            let changed = [
                node.tag != old.tag || node.attrs != old.attrs || node.inline != old.inline,
                node.parent != old.parent,
                node.children != old.children,
                node.text.is_empty() != old.text.is_empty(),
                node.tag != old.tag,
                node.state != old.state,
            ];
            if changed.iter().any(|&value| value) {
                for (kind, (revision, changed)) in revisions.iter_mut().zip(changed).enumerate() {
                    if changed {
                        *revision += 1;
                        changes.push(id * 6 + kind);
                    }
                }
                *old = Node {
                    tag: node.tag.clone(),
                    attrs: node.attrs.clone(),
                    inline: node.inline.clone(),
                    parent: node.parent,
                    children: node.children.clone(),
                    state: node.state,
                    text: if node.text.is_empty() {
                        String::new()
                    } else {
                        "x".into()
                    },
                    ..Default::default()
                };
            }
        }
        if !changes.is_empty() {
            self.epoch += 1;
            self.changes.push_back((self.epoch, changes));
            if self.changes.len() > 32 {
                self.changes.pop_front();
            }
        }
    }

    pub(crate) fn depend(&mut self, id: usize, kind: usize) {
        if let Some((_, revisions)) = self.nodes.get(id) {
            self.dependencies.push((id * 6 + kind, revisions[kind]));
        }
    }

    pub(crate) fn valid(&self, dependencies: &[(usize, u64)]) -> bool {
        dependencies.iter().all(|(id, revision)| {
            self.nodes
                .get(*id / 6)
                .map_or(0, |(_, current)| current[*id % 6])
                == *revision
        })
    }

    pub(crate) fn valid_since(
        &self,
        dependencies: &[(usize, u64)],
        checked: &std::cell::Cell<u64>,
    ) -> bool {
        let since = checked.get();
        if since == self.epoch {
            return true;
        }
        let valid = if self
            .changes
            .front()
            .is_some_and(|(epoch, _)| since + 1 >= *epoch)
            && self.changes.back().is_some_and(|(_, changes)| {
                dependencies.len()
                    > changes.len() * (dependencies.len().max(1).ilog2() as usize + 1)
            }) {
            self.changes
                .iter()
                .rev()
                .take_while(|(epoch, _)| *epoch > since)
                .all(|(_, changes)| {
                    changes.iter().all(|id| {
                        match dependencies.binary_search_by_key(id, |(id, _)| *id) {
                            Ok(index) => self.valid(&dependencies[index..index + 1]),
                            Err(_) => true,
                        }
                    })
                })
        } else {
            self.valid(dependencies)
        };
        if valid {
            checked.set(self.epoch);
        }
        valid
    }
}

impl Selector {
    fn argument(name: &str, argument: &str) -> Option<(Vec<Self>, [i32; 2])> {
        let list = |source: &str| Some(Self::parse(source, None)).filter(|list| !list.is_empty());
        match name {
            "is" | "where" => Some((
                split(argument)
                    .into_iter()
                    .flat_map(|part| Self::parse(part, None))
                    .collect(),
                [0, 0],
            )),
            "not" => Some((list(argument)?, [0, 0])),
            "host" if !argument.is_empty() => Some((list(argument)?, [0, 0])),
            "has" => {
                let relative = split(argument)
                    .into_iter()
                    .map(|part| {
                        (!part.trim().is_empty()).then(|| format!(":scope {}", part.trim()))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some((list(&relative.join(","))?, [0, 0]))
            }
            "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                let of = argument.as_bytes().windows(4).position(|window| {
                    window[0].is_ascii_whitespace()
                        && window[1..3].eq_ignore_ascii_case(b"of")
                        && window[3].is_ascii_whitespace()
                });
                let (formula, selectors) = match of {
                    Some(_) if name.ends_with("of-type") => return None,
                    Some(index) => (&argument[..index], list(&argument[index + 4..])?),
                    None => (argument, Vec::new()),
                };
                let formula = formula.replace("/**/", " ").trim().to_ascii_lowercase();
                let nth = match formula.as_str() {
                    "odd" => [2, 1],
                    "even" => [2, 0],
                    _ => match formula.split_once('n') {
                        None => [0, formula.parse().ok()?],
                        Some((a, b)) => {
                            let b = b.trim_start();
                            let (negative, digits) = match b.as_bytes().first() {
                                None => (false, "0"),
                                Some(b'+') => (false, b[1..].trim_start()),
                                Some(b'-') => (true, b[1..].trim_start()),
                                _ => return None,
                            };
                            if !digits.starts_with(|c: char| c.is_ascii_digit()) {
                                return None;
                            }
                            let b: i32 = digits.parse().ok()?;
                            [
                                match a {
                                    "" | "+" => 1,
                                    "-" => -1,
                                    _ => a.parse().ok()?,
                                },
                                if negative { -b } else { b },
                            ]
                        }
                    },
                };
                Some((selectors, nth))
            }
            "lang" | "dir" => (!argument.trim().is_empty()).then(|| (Vec::new(), [0, 0])),
            _ => Some((Vec::new(), [0, 0])),
        }
    }

    pub(crate) fn bind_parent(&mut self, parent: Option<&SelectorList>) -> anyhow::Result<()> {
        for selector in &mut self.simple {
            match selector {
                SimpleSelector::Parent(selectors, _) => {
                    *selectors = parent
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing CSS parent selector"))?;
                }
                SimpleSelector::Pseudo(name, argument, parsed, _) => {
                    if let Some((selectors, _)) = Self::argument(name, argument) {
                        *parsed = selectors;
                    }
                }
                _ => {}
            }
        }
        if let Some((_, previous)) = &mut self.previous {
            previous.bind_parent(parent)?;
        }
        Ok(())
    }

    pub(crate) fn without_pseudo_element(&self) -> Option<(usize, Self)> {
        let (index, kind) = self.simple.iter().enumerate().find_map(|(index, part)| {
            let SimpleSelector::Element(name, _) = part else {
                return None;
            };
            Some((
                index,
                [
                    "before",
                    "after",
                    "selection",
                    "marker",
                    "first-letter",
                    "first-line",
                ]
                .iter()
                .position(|n| n == name)?,
            ))
        })?;
        let mut selector = self.clone();
        selector.simple.remove(index);
        Some((kind, selector))
    }

    pub(crate) fn key(&self) -> Option<(u8, &str)> {
        self.simple
            .iter()
            .filter_map(|selector| match selector {
                SimpleSelector::Id(id) => Some((0, id.as_str())),
                SimpleSelector::Class(class) => Some((1, class.as_str())),
                SimpleSelector::Tag(tag) => Some((2, tag.as_str())),
                SimpleSelector::Attribute(name, ..) => Some((3, name.as_str())),
                _ => None,
            })
            .min_by_key(|(kind, _)| *kind)
            .or_else(|| {
                self.simple.iter().find_map(|part| {
                    let alternatives = match part {
                        SimpleSelector::Parent(selectors, _) => selectors.as_slice(),
                        SimpleSelector::Pseudo(name, _, selectors, _)
                            if matches!(name.as_str(), "is" | "where") =>
                        {
                            selectors.as_slice()
                        }
                        _ => return None,
                    };
                    let key = alternatives.first()?.key()?;
                    alternatives
                        .iter()
                        .all(|selector| selector.key() == Some(key))
                        .then_some(key)
                })
            })
    }

    pub(crate) fn source(&self) -> String {
        let escape = |source: &mut String, value: &str| {
            for (index, c) in value.char_indices() {
                if c.is_ascii_control()
                    || c == ' '
                    || c.is_ascii_digit() && (index == 0 || index == 1 && value.starts_with('-'))
                {
                    source.push_str(&format!("\\{:x} ", c as u32));
                } else {
                    if !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii()) {
                        source.push('\\');
                    }
                    source.push(c);
                }
            }
        };
        let mut source = String::new();
        if let Some((combinator, previous)) = &self.previous {
            source.push_str(&previous.source());
            source.push(*combinator);
        }
        for selector in &self.simple {
            match selector {
                SimpleSelector::Tag(tag) => escape(&mut source, tag),
                SimpleSelector::Id(id) => {
                    source.push('#');
                    escape(&mut source, id);
                }
                SimpleSelector::Class(class) => {
                    source.push('.');
                    escape(&mut source, class);
                }
                SimpleSelector::Universal => source.push('*'),
                SimpleSelector::Root => source.push_str(":root"),
                SimpleSelector::Attribute(name, op, value, insensitive) => {
                    source.push('[');
                    escape(&mut source, name);
                    source.push_str(op);
                    if !op.is_empty() {
                        source.push('"');
                        source.push_str(
                            &value
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                                .replace('\n', "\\a "),
                        );
                        source.push('"');
                    }
                    if *insensitive {
                        source.push_str(" i");
                    }
                    source.push(']');
                }
                SimpleSelector::Pseudo(name, argument, ..)
                | SimpleSelector::Element(name, argument) => {
                    source.push(':');
                    if matches!(selector, SimpleSelector::Element(..)) {
                        source.push(':');
                    }
                    source.push_str(name);
                    if !argument.is_empty() {
                        source.push('(');
                        source.push_str(argument);
                        source.push(')');
                    }
                }
                SimpleSelector::Parent(_, count) => source.extend(std::iter::repeat_n('&', *count)),
            }
        }
        source
    }

    pub(crate) fn parse(source: &str, parent: Option<&SelectorList>) -> Vec<Self> {
        let mut depth = 0u32;
        if source.bytes().any(|c| {
            depth = match c {
                b'(' => depth + 1,
                b')' => depth.saturating_sub(1),
                _ => depth,
            };
            depth > 32
        }) {
            return Vec::new();
        }
        let mut selectors = Vec::new();
        let parent_specificity = parent
            .and_then(|p| p.iter().map(|s| s.specificity).max())
            .unwrap_or_default();
        for source in split(source) {
            let mut rest = source.trim();
            if rest.is_empty() {
                return Vec::new();
            }
            let mut previous = parent
                .filter(|_| !rest.contains('&') || rest.starts_with(['>', '+', '~']))
                .map(|parent| Self {
                    simple: vec![SimpleSelector::Parent(Rc::clone(parent), 1)],
                    previous: None,
                    specificity: parent_specificity,
                });
            let mut compounds = 0;
            while !rest.is_empty() {
                // WARNING: matching recurses per compound, so long chains can overflow the stack.
                compounds += 1;
                if compounds > 256 {
                    return Vec::new();
                }
                let mut combinator = ' ';
                if rest.starts_with(['>', '+', '~']) {
                    if previous.is_none() {
                        return Vec::new();
                    }
                    combinator = rest.as_bytes()[0] as char;
                    rest = rest[1..].trim_start();
                }
                let end = selector_boundary(rest, |c| {
                    c.is_ascii_whitespace() || matches!(c, '>' | '+' | '~')
                });
                if end == 0 {
                    return Vec::new();
                }
                let mut compound = &rest[..end];
                let mut simple = Vec::new();
                let mut parent_index = None;
                let mut specificity = previous.as_ref().map_or([0; 3], |s| s.specificity);
                while !compound.is_empty() {
                    let marker = compound.as_bytes()[0];
                    if marker == b'[' {
                        let end = 1 + selector_boundary(&compound[1..], |c| c == ']');
                        if end == compound.len() {
                            return Vec::new();
                        }
                        let attribute = compound[1..end].trim();
                        let attribute = attribute
                            .strip_prefix("*|")
                            .or_else(|| {
                                attribute
                                    .strip_prefix('|')
                                    .filter(|rest| !rest.starts_with('='))
                            })
                            .unwrap_or(attribute);
                        let Some((name, length)) = identifier(attribute) else {
                            return Vec::new();
                        };
                        let tail = attribute[length..].trim_start();
                        let (operator, value, insensitive) = if tail.is_empty() {
                            ("", String::new(), false)
                        } else {
                            let Some(operator) = ["=", "~=", "|=", "^=", "$=", "*="]
                                .into_iter()
                                .find(|operator| tail.starts_with(operator))
                            else {
                                return Vec::new();
                            };
                            let tail = tail[operator.len()..].trim_start();
                            let (value, length) = if let Some(quote) =
                                tail.chars().next().filter(|c| matches!(c, '"' | '\''))
                            {
                                let (value, length) = unescape(&tail[1..], |c| c == quote);
                                if length + 1 == tail.len() {
                                    return Vec::new();
                                }
                                (value, length + 2)
                            } else if let Some(value) = identifier(tail) {
                                value
                            } else {
                                return Vec::new();
                            };
                            let insensitive = match tail[length..].trim_start() {
                                "" | "s" | "S" => false,
                                "i" | "I" => true,
                                _ => return Vec::new(),
                            };
                            (
                                operator,
                                if insensitive {
                                    value.to_ascii_lowercase()
                                } else {
                                    value
                                },
                                insensitive,
                            )
                        };
                        simple.push(SimpleSelector::Attribute(
                            name,
                            operator.into(),
                            value,
                            insensitive,
                        ));
                        specificity[1] = specificity[1].saturating_add(1);
                        compound = &compound[end + 1..];
                        continue;
                    }
                    if matches!(marker, b'&' | b'*') {
                        if marker == b'*' && !simple.is_empty() {
                            return Vec::new();
                        }
                        if marker == b'&' {
                            if let Some(parent) = parent {
                                if let Some(index) = parent_index {
                                    if let SimpleSelector::Parent(_, count) = &mut simple[index] {
                                        *count += 1;
                                    }
                                } else {
                                    parent_index = Some(simple.len());
                                    simple.push(SimpleSelector::Parent(Rc::clone(parent), 1));
                                }
                                for (value, inherited) in
                                    specificity.iter_mut().zip(parent_specificity)
                                {
                                    *value = value.saturating_add(inherited);
                                }
                            } else {
                                simple.push(SimpleSelector::Root);
                            }
                        } else {
                            simple.push(SimpleSelector::Universal);
                        }
                        compound = &compound[1..];
                        continue;
                    }
                    if marker == b':' {
                        let element = compound.starts_with("::");
                        compound = &compound[if element { 2 } else { 1 }..];
                        let Some((name, length)) = identifier(compound) else {
                            return Vec::new();
                        };
                        let name = name.to_ascii_lowercase();
                        compound = &compound[length..];
                        let mut argument = "";
                        if compound.starts_with('(') {
                            let close = 1 + selector_boundary(&compound[1..], |c| c == ')');
                            if close == compound.len() {
                                return Vec::new();
                            }
                            argument = &compound[1..close];
                            compound = &compound[close + 1..];
                        }
                        if element
                            || matches!(
                                name.as_str(),
                                "before" | "after" | "first-line" | "first-letter"
                            )
                        {
                            specificity[2] = specificity[2].saturating_add(1);
                            simple.push(SimpleSelector::Element(name, argument.into()));
                            continue;
                        }
                        if name == "root" {
                            specificity[1] = specificity[1].saturating_add(1);
                            simple.push(SimpleSelector::Root);
                            continue;
                        }
                        let Some((selectors, nth)) = Self::argument(&name, argument) else {
                            return Vec::new();
                        };
                        let [ids, classes, tags] = selectors
                            .iter()
                            .map(|selector| selector.specificity)
                            .max()
                            .unwrap_or_default();
                        let own = match name.as_str() {
                            "where" => [0; 3],
                            "is" | "not" => [ids, classes, tags],
                            "has" => [ids, classes.saturating_sub(1), tags],
                            _ => [ids, classes.saturating_add(1), tags],
                        };
                        for (value, own) in specificity.iter_mut().zip(own) {
                            *value = value.saturating_add(own);
                        }
                        simple.push(SimpleSelector::Pseudo(
                            name,
                            argument.into(),
                            selectors,
                            nth,
                        ));
                        continue;
                    }
                    if matches!(marker, b'.' | b'#') {
                        compound = &compound[1..];
                    } else if !simple.is_empty() {
                        return Vec::new();
                    }
                    let Some((name, length)) = identifier(compound) else {
                        return Vec::new();
                    };
                    compound = &compound[length..];
                    simple.push(match marker {
                        b'#' => {
                            specificity[0] = specificity[0].saturating_add(1);
                            SimpleSelector::Id(name)
                        }
                        b'.' => {
                            specificity[1] = specificity[1].saturating_add(1);
                            SimpleSelector::Class(name)
                        }
                        _ => {
                            specificity[2] = specificity[2].saturating_add(1);
                            SimpleSelector::Tag(name)
                        }
                    });
                }
                previous = Some(Self {
                    simple,
                    previous: previous.map(|s| (combinator, Box::new(s))),
                    specificity,
                });
                rest = rest[end..].trim_start();
            }
            selectors.push(previous.unwrap());
        }
        selectors
    }

    pub(crate) fn matches(&self, doc: &Document, id: usize) -> bool {
        self.matches_scope(doc, id, None)
    }

    fn matches_scope(&self, doc: &Document, id: usize, scope: Option<usize>) -> bool {
        self.matches_cached(doc, id, scope, &mut SelectorCache::default())
    }

    pub(crate) fn matches_reusing(
        &self,
        doc: &Document,
        id: usize,
        memo: &mut SelectorCache,
    ) -> bool {
        self.matches_cached(doc, id, None, memo)
    }

    pub(crate) fn matches_cached(
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
    ) -> bool {
        let key = (self as *const Self as usize, id, scope, false);
        let compound = self.previous.is_none()
            && self.simple.iter().all(|simple| match simple {
                SimpleSelector::Pseudo(_, _, parsed, _) => parsed.is_empty(),
                SimpleSelector::Parent(..) => false,
                _ => true,
            });
        if !compound
            && let Some((result, dependencies)) = memo.values.get(&key)
            && memo.valid(dependencies)
        {
            memo.dependencies.extend(dependencies.iter().copied());
            return *result;
        }
        let start = memo.dependencies.len();
        memo.depend(id, 0);
        let result = (|| {
            let node = &doc.nodes[id];
            if node.tag.starts_with('#')
                || !self.simple.iter().all(|s| match s {
                    SimpleSelector::Tag(tag) => node.tag == *tag,
                    SimpleSelector::Id(id) => node.attrs.get("id") == Some(id),
                    SimpleSelector::Class(class) => {
                        node.attrs.get("class").is_some_and(|classes| {
                            classes.split_ascii_whitespace().any(|c| c == class)
                        })
                    }
                    SimpleSelector::Universal => true,
                    SimpleSelector::Root => node.tag == "html",
                    SimpleSelector::Attribute(name, operator, value, insensitive) => {
                        node.attrs.get(name).is_some_and(|actual| {
                            let lower;
                            let actual = if *insensitive {
                                lower = actual.to_ascii_lowercase();
                                &lower
                            } else {
                                actual
                            };
                            match operator.as_str() {
                                "" => true,
                                "=" => actual == value,
                                "~=" => actual.split_ascii_whitespace().any(|word| word == value),
                                "|=" => actual
                                    .strip_prefix(value.as_str())
                                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-')),
                                _ if value.is_empty() => false,
                                "^=" => actual.starts_with(value.as_str()),
                                "$=" => actual.ends_with(value.as_str()),
                                "*=" => actual.contains(value.as_str()),
                                _ => false,
                            }
                        })
                    }
                    SimpleSelector::Pseudo(name, argument, parsed, [a, b]) => match name.as_str() {
                        "scope" => scope.map_or(node.tag == "html", |scope| scope == id),
                        "has" => {
                            memo.depend(id, 2);
                            let mut pending = node.children.clone();
                            if argument
                                .split(',')
                                .any(|part| part.trim_start().starts_with(['+', '~']))
                                && let Some(parent) = node.parent
                            {
                                memo.depend(id, 1);
                                memo.depend(parent, 2);
                                pending.extend(
                                    doc.nodes[parent]
                                        .children
                                        .iter()
                                        .skip_while(|&&sibling| sibling != id)
                                        .skip(1),
                                );
                            }
                            let mut found = false;
                            while let Some(child) = pending.pop() {
                                if parsed.iter().any(|selector| {
                                    selector.matches_cached(doc, child, Some(id), memo)
                                }) {
                                    found = true;
                                    break;
                                }
                                memo.depend(child, 2);
                                pending.extend(&doc.nodes[child].children);
                            }
                            found
                        }
                        "host" => {
                            node.tag == "apps-shadow-root"
                                && (parsed.is_empty()
                                    || parsed
                                        .iter()
                                        .any(|s| s.matches_cached(doc, id, scope, memo)))
                        }
                        "empty" => {
                            memo.depend(id, 2);
                            memo.depend(id, 3);
                            for &child in &node.children {
                                memo.depend(child, 4);
                                memo.depend(child, 3);
                            }
                            node.text.is_empty()
                                && node.children.iter().all(|&id| {
                                    doc.nodes[id].tag == "#comment"
                                        || doc.nodes[id].tag == "#text"
                                            && doc.nodes[id].text.is_empty()
                                })
                        }
                        "hover" | "active" | "focus" | "focus-visible" | "focus-within" => {
                            memo.depend(id, 5);
                            node.state
                                & match name.as_str() {
                                    "hover" => HOVER,
                                    "active" => ACTIVE,
                                    "focus-within" => FOCUS_WITHIN,
                                    _ => FOCUS,
                                }
                                != 0
                        }
                        "link" | "any-link" | "-webkit-any-link" => {
                            matches!(node.tag.as_str(), "a" | "area")
                                && node.attrs.contains_key("href")
                        }
                        "defined" => true,
                        "open" => {
                            matches!(node.tag.as_str(), "details" | "dialog")
                                && node.attrs.contains_key("open")
                        }
                        "indeterminate" => {
                            node.tag == "progress" && !node.attrs.contains_key("value")
                        }
                        "checked" => match node.tag.as_str() {
                            "input" => {
                                node.attrs.contains_key("checked")
                                    && node.attrs.get("type").is_some_and(|kind| {
                                        kind.eq_ignore_ascii_case("checkbox")
                                            || kind.eq_ignore_ascii_case("radio")
                                    })
                            }
                            "option" => node.attrs.contains_key("selected"),
                            _ => false,
                        },
                        "disabled" | "enabled" => {
                            matches!(
                                node.tag.as_str(),
                                "button"
                                    | "input"
                                    | "select"
                                    | "textarea"
                                    | "optgroup"
                                    | "option"
                                    | "fieldset"
                            ) && node.attrs.contains_key("disabled") == (name == "disabled")
                        }
                        "placeholder-shown" | "required" | "optional" | "valid" | "invalid" => {
                            memo.depend(id, 2);
                            memo.depend(id, 3);
                            let required = node.attrs.contains_key("required");
                            let empty = if node.attrs.get("type").is_some_and(|kind| {
                                kind.eq_ignore_ascii_case("checkbox")
                                    || kind.eq_ignore_ascii_case("radio")
                            }) {
                                !node.attrs.contains_key("checked")
                            } else {
                                node.attrs.get("value").map_or(
                                    node.text.is_empty() && node.children.is_empty(),
                                    String::is_empty,
                                )
                            };
                            matches!(node.tag.as_str(), "input" | "select" | "textarea")
                                && match name.as_str() {
                                    "placeholder-shown" => {
                                        node.tag != "select"
                                            && node.attrs.contains_key("placeholder")
                                            && empty
                                    }
                                    "required" => required,
                                    "optional" => !required,
                                    "invalid" => required && empty,
                                    _ => !(required && empty),
                                }
                        }
                        "read-write" | "read-only"
                            if matches!(node.tag.as_str(), "input" | "textarea") =>
                        {
                            let writable = !node.attrs.contains_key("readonly")
                                && !node.attrs.contains_key("disabled")
                                && (node.tag == "textarea"
                                    || node.attrs.get("type").is_none_or(|kind| {
                                        ![
                                            "hidden", "checkbox", "radio", "file", "submit",
                                            "image", "reset", "button", "color", "range",
                                        ]
                                        .iter()
                                        .any(|other| kind.eq_ignore_ascii_case(other))
                                    }));
                            writable == (name == "read-write")
                        }
                        "read-write" | "read-only" | "lang" | "dir" => {
                            let attribute = match name.as_str() {
                                "lang" | "dir" => name.as_str(),
                                _ => "contenteditable",
                            };
                            let mut value = None;
                            let mut current = Some(id);
                            while value.is_none()
                                && let Some(ancestor) = current
                            {
                                memo.depend(ancestor, 0);
                                memo.depend(ancestor, 1);
                                value = doc.nodes[ancestor].attrs.get(attribute).filter(|value| {
                                    match attribute {
                                        "lang" => true,
                                        "dir" => ["ltr", "rtl", "auto"]
                                            .iter()
                                            .any(|v| value.eq_ignore_ascii_case(v)),
                                        _ => ["", "true", "false", "plaintext-only"]
                                            .iter()
                                            .any(|v| value.eq_ignore_ascii_case(v)),
                                    }
                                });
                                current = doc.nodes[ancestor].parent;
                            }
                            match name.as_str() {
                                "lang" => value.is_some_and(|value| {
                                    !value.is_empty()
                                        && argument.split(',').any(|range| {
                                            let range = range.trim().trim_matches(['"', '\'']);
                                            range == "*"
                                                || value.as_bytes().get(..range.len()).is_some_and(
                                                    |prefix| {
                                                        prefix
                                                            .eq_ignore_ascii_case(range.as_bytes())
                                                    },
                                                ) && value
                                                    .as_bytes()
                                                    .get(range.len())
                                                    .is_none_or(|&c| c == b'-')
                                        })
                                }),
                                "dir" => {
                                    let rtl = value
                                        .is_some_and(|value| value.eq_ignore_ascii_case("rtl"));
                                    let wanted = argument.trim();
                                    wanted.eq_ignore_ascii_case("rtl") && rtl
                                        || wanted.eq_ignore_ascii_case("ltr") && !rtl
                                }
                                _ => {
                                    value.is_some_and(|value| !value.eq_ignore_ascii_case("false"))
                                        == (name == "read-write")
                                }
                            }
                        }
                        "not" => !parsed
                            .iter()
                            .any(|s| s.matches_cached(doc, id, scope, memo)),
                        "is" | "where" => parsed
                            .iter()
                            .any(|s| s.matches_cached(doc, id, scope, memo)),
                        "first-child" | "last-child" | "only-child" | "nth-child"
                        | "nth-last-child" | "first-of-type" | "last-of-type" | "only-of-type"
                        | "nth-of-type" | "nth-last-of-type" => {
                            memo.depend(id, 1);
                            node.parent.is_some_and(|parent| {
                                memo.depend(parent, 2);
                                for &child in &doc.nodes[parent].children {
                                    memo.depend(child, 4);
                                }
                                if !parsed.is_empty()
                                    && !parsed
                                        .iter()
                                        .any(|s| s.matches_cached(doc, id, scope, memo))
                                {
                                    return false;
                                }
                                let of_type = name.ends_with("of-type");
                                let mut siblings =
                                    doc.nodes[parent].children.iter().filter(|&&child| {
                                        let tag = &doc.nodes[child].tag;
                                        !tag.starts_with('#')
                                            && (!of_type || *tag == node.tag)
                                            && (parsed.is_empty()
                                                || child == id
                                                || parsed.iter().any(|s| {
                                                    s.matches_cached(doc, child, scope, memo)
                                                }))
                                    });
                                match name.as_str() {
                                    "first-child" | "first-of-type" => siblings.next() == Some(&id),
                                    "last-child" | "last-of-type" => {
                                        siblings.next_back() == Some(&id)
                                    }
                                    "only-child" | "only-of-type" => siblings.nth(1).is_none(),
                                    _ => if name.starts_with("nth-last") {
                                        siblings.rev().position(|&child| child == id)
                                    } else {
                                        siblings.position(|&child| child == id)
                                    }
                                    .is_some_and(|index| {
                                        let (a, n) = (*a as i64, index as i64 + 1 - *b as i64);
                                        if a == 0 {
                                            n == 0
                                        } else {
                                            n % a == 0 && n / a >= 0
                                        }
                                    }),
                                }
                            })
                        }
                        _ => false,
                    },
                    SimpleSelector::Element(..) => false,
                    SimpleSelector::Parent(parent, _) => parent
                        .iter()
                        .any(|s| s.matches_cached(doc, id, scope, memo)),
                })
            {
                return false;
            }
            let Some((combinator, previous)) = &self.previous else {
                return true;
            };
            memo.depend(id, 1);
            let Some(parent) = node.parent else {
                return false;
            };
            match combinator {
                '>' => previous.matches_cached(doc, parent, scope, memo),
                '+' | '~' => {
                    memo.depend(parent, 2);
                    for &child in &doc.nodes[parent].children {
                        memo.depend(child, 4);
                    }
                    let mut siblings = doc.nodes[parent]
                        .children
                        .iter()
                        .rev()
                        .skip_while(|&&sibling| sibling != id)
                        .skip(1)
                        .filter(|&&sibling| !doc.nodes[sibling].tag.starts_with('#'));
                    if *combinator == '+' {
                        siblings
                            .next()
                            .is_some_and(|&id| previous.matches_cached(doc, id, scope, memo))
                    } else {
                        siblings.any(|&id| previous.matches_cached(doc, id, scope, memo))
                    }
                }
                _ => {
                    let mut ancestor = Some(parent);
                    let mut visited = Vec::new();
                    let start = memo.dependencies.len();
                    let mut matched = false;
                    while let Some(id) = ancestor {
                        memo.depend(id, 1);
                        let key = (&**previous as *const Self as usize, id, scope, true);
                        if let Some((result, dependencies)) = memo.values.get(&key)
                            && memo.valid(dependencies)
                        {
                            matched = *result;
                            memo.dependencies.extend(dependencies.iter().copied());
                            break;
                        }
                        visited.push(key);
                        if previous.matches_cached(doc, id, scope, memo) {
                            matched = true;
                            break;
                        }
                        ancestor = doc.nodes[id].parent;
                    }
                    let mut dependencies = memo.dependencies[start..].to_vec();
                    dependencies.sort_unstable();
                    dependencies.dedup();
                    memo.dependencies.truncate(start);
                    memo.dependencies.extend(&dependencies);
                    let dependencies: Rc<[_]> = dependencies.into();
                    for key in visited {
                        memo.values.insert(key, (matched, dependencies.clone()));
                    }
                    matched
                }
            }
        })();
        // A compound without combinators or nested lists is as cheap to match again as to look up.
        if compound {
            return result;
        }
        let mut dependencies = memo.dependencies[start..].to_vec();
        dependencies.sort_unstable();
        dependencies.dedup();
        memo.dependencies.truncate(start);
        memo.dependencies.extend(&dependencies);
        memo.values.insert(key, (result, dependencies.into()));
        result
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Node {
    pub tag: String,
    pub text: String,
    #[serde(with = "crate::bundle::map")]
    pub attrs: BBTreeMap<String, String>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub(crate) stylesheet: Option<Rc<crate::render::Stylesheet>>,
    pub(crate) inline: Option<crate::render::Declarations>,
    #[serde(skip)]
    pub(crate) state: u8,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Document {
    pub nodes: Vec<Node>,
}

impl Document {
    pub fn parse(source: &str) -> Self {
        let mut doc = Self {
            nodes: vec![Node {
                tag: "#document".into(),
                ..Node::default()
            }],
        };
        let mut stack = vec![(0, false)];
        let mut pos = 0;
        while pos < source.len() {
            let rest = &source[pos..];
            if rest.starts_with("<!--") {
                pos += rest.find("-->").map_or(rest.len(), |n| n + 3);
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                pos += rest.find('>').map_or(rest.len(), |n| n + 1);
                continue;
            }
            if let Some(close) = rest.strip_prefix("</") {
                let end = close.find('>').unwrap_or(close.len());
                let tag = close[..end].trim().to_ascii_lowercase();
                for index in (1..stack.len()).rev() {
                    let node = &doc.nodes[stack[index].0];
                    if node.tag.eq_ignore_ascii_case(&tag) {
                        stack.truncate(index);
                        break;
                    }
                    if matches!(
                        node.tag.as_str(),
                        "html" | "table" | "td" | "th" | "template" | "select"
                    ) {
                        break;
                    }
                }
                pos += (end + 3).min(rest.len());
                continue;
            }
            let (parent, in_svg) = *stack.last().unwrap();
            let mut node = Node {
                parent: Some(parent),
                ..Node::default()
            };
            if rest.starts_with('<') && rest.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            {
                let mut end = 1;
                let mut quote = 0;
                for (i, byte) in rest.bytes().enumerate().skip(1) {
                    if quote != 0 {
                        if byte == quote {
                            quote = 0;
                        }
                    } else if byte == b'\'' || byte == b'"' {
                        quote = byte;
                    } else if byte == b'>' {
                        end = i;
                        break;
                    }
                    end = i + 1;
                }
                let inside = &rest[1..end];
                let name_end = inside
                    .find(|c: char| c.is_ascii_whitespace() || c == '/')
                    .unwrap_or(inside.len());
                node.tag = inside[..name_end].to_ascii_lowercase();
                let svg = in_svg || node.tag == "svg";
                if svg {
                    node.tag = inside[..name_end].to_owned();
                }
                let mut attrs = inside[name_end..].trim();
                while !attrs.is_empty() && attrs != "/" {
                    let split = attrs
                        .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
                        .unwrap_or(attrs.len());
                    if split == 0 {
                        break;
                    }
                    let name = if svg {
                        attrs[..split].to_owned()
                    } else {
                        attrs[..split].to_ascii_lowercase()
                    };
                    attrs = attrs[split..].trim_start();
                    let mut value = "";
                    if let Some(tail) = attrs.strip_prefix('=') {
                        attrs = tail.trim_start();
                        if attrs.starts_with(['\'', '"']) {
                            let quote = attrs.as_bytes()[0] as char;
                            attrs = &attrs[1..];
                            let length = attrs.find(quote).unwrap_or(attrs.len());
                            value = &attrs[..length];
                            attrs = &attrs[(length + 1).min(attrs.len())..];
                        } else {
                            let length = attrs.find(char::is_whitespace).unwrap_or(attrs.len());
                            value = &attrs[..length];
                            attrs = &attrs[length..];
                        }
                    }
                    if !node.attrs.contains_key(&name) {
                        node.attrs.insert(name, decode(value));
                    }
                    attrs = attrs.trim_start();
                }
                pos += (end + 1).min(rest.len());
                let id = doc.nodes.len();
                let void = matches!(
                    node.tag.as_str(),
                    "area"
                        | "base"
                        | "br"
                        | "col"
                        | "embed"
                        | "hr"
                        | "img"
                        | "input"
                        | "link"
                        | "meta"
                        | "param"
                        | "source"
                        | "track"
                        | "wbr"
                );
                let raw = matches!(node.tag.as_str(), "script" | "style" | "title" | "textarea");
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                if raw {
                    let closing = format!("</{}", doc.nodes[id].tag);
                    let tail = &source[pos..];
                    let length = tail
                        .as_bytes()
                        .windows(closing.len())
                        .enumerate()
                        .find(|(offset, w)| {
                            w.eq_ignore_ascii_case(closing.as_bytes())
                                && tail
                                    .as_bytes()
                                    .get(offset + closing.len())
                                    .is_none_or(|c| c.is_ascii_whitespace() || *c == b'>')
                        })
                        .map(|(offset, _)| offset)
                        .unwrap_or(tail.len());
                    doc.nodes[id].text =
                        if matches!(doc.nodes[id].tag.as_str(), "title" | "textarea") {
                            decode(&tail[..length])
                        } else {
                            tail[..length].into()
                        };
                    pos += length;
                    if let Some(end) = source[pos..].find('>') {
                        pos += end + 1;
                    }
                } else if !void && !(svg && inside.trim_end().ends_with('/')) {
                    stack.push((id, svg));
                }
            } else {
                let first = rest.chars().next().unwrap().len_utf8();
                let length = rest[first..].find('<').map_or(rest.len(), |n| n + first);
                node.tag = "#text".into();
                node.text = decode(&rest[..length]);
                let id = doc.nodes.len();
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                pos += length;
            }
        }
        doc
    }

    pub fn title(&self) -> Option<String> {
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id];
            if node.tag == "title" {
                return Some(
                    self.text(id)
                        .split_ascii_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            pending.extend(node.children.iter().rev());
        }
        None
    }

    pub fn text(&self, id: usize) -> String {
        let mut text = String::new();
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id];
            match &node.stylesheet {
                Some(sheet) => text.push_str(&sheet.source()),
                None => text.push_str(&node.text),
            }
            pending.extend(node.children.iter().rev());
        }
        text
    }

    pub fn serialize(&self, id: usize) -> String {
        let mut result = String::new();
        let mut pending = vec![(id, false)];
        while let Some((id, close)) = pending.pop() {
            let node = &self.nodes[id];
            if close {
                result.push_str("</");
                result.push_str(&node.tag);
                result.push('>');
                continue;
            }
            match node.tag.as_str() {
                "#text" => {
                    result.push_str(&escape(&node.text));
                    continue;
                }
                "#comment" => continue,
                _ => {}
            }
            let element = id != 0 && !node.tag.starts_with('#');
            if element {
                result.push('<');
                result.push_str(&node.tag);
                for (key, value) in &node.attrs {
                    result.push(' ');
                    result.push_str(key);
                    result.push_str("=\"");
                    result.push_str(&escape(value));
                    result.push('"');
                }
                if let Some(inline) = &node.inline {
                    result.push_str(" style=\"");
                    result.push_str(&escape(&inline.source()));
                    result.push('"');
                }
                result.push('>');
            }
            if matches!(node.tag.as_str(), "script" | "style") {
                if let Some(sheet) = &node.stylesheet {
                    result.push_str(&sheet.source());
                } else {
                    result.push_str(&node.text);
                }
            } else {
                result.push_str(&escape(&node.text));
            }
            if element
                && !matches!(
                    node.tag.as_str(),
                    "area"
                        | "base"
                        | "br"
                        | "col"
                        | "embed"
                        | "hr"
                        | "img"
                        | "input"
                        | "link"
                        | "meta"
                        | "param"
                        | "source"
                        | "track"
                        | "wbr"
                )
            {
                pending.push((id, true));
            }
            pending.extend(node.children.iter().rev().map(|&child| (child, false)));
        }
        result
    }

    pub fn matches(&self, id: usize, selector: &str) -> bool {
        Selector::parse(selector, None)
            .iter()
            .any(|selector| selector.matches(self, id))
    }
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn decode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|n| *n < 16) else {
            result.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let character = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(character) = character {
            result.push(character);
        } else {
            result.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendant_failures_are_bounded() {
        let doc = Document::parse(&format!(
            "{}<span></span>{}",
            "<div class='a'>".repeat(1000),
            "</div>".repeat(1000)
        ));
        let selector = Selector::parse(&format!(".missing {}span", ".a ".repeat(29)), None);
        let leaf = doc
            .nodes
            .iter()
            .position(|node| node.tag == "span")
            .unwrap();
        let started = std::time::Instant::now();
        assert!(!selector[0].matches(&doc, leaf));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn closing_tags_do_not_cross_table_scope() {
        let doc = Document::parse(
            "<div><table><tr><td></div><span>inside</span></td></tr></table></div>",
        );
        let span = doc
            .nodes
            .iter()
            .position(|node| node.tag == "span")
            .unwrap();
        assert_eq!(doc.nodes[doc.nodes[span].parent.unwrap()].tag, "td");
    }

    #[test]
    fn has_handles_relative_selectors_and_nested_lists() {
        let doc = Document::parse(
            "<main><code><div><i class='separator'></i></div></code></main><main id='empty'></main>",
        );
        let id = doc.nodes.iter().position(|n| n.tag == "main").unwrap();
        assert!(doc.matches(id, ":has(>code > * > :is(.annotation,.separator))"));
        assert!(doc.matches(id, ":not(:has(> input))"));
        assert!(!doc.matches(id, ":not(:has(>code > * > :is(.annotation,.separator)))"));
        let empty = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|v| v == "empty"))
            .unwrap();
        assert!(!doc.matches(empty, ":has(.separator)"));
        assert!(doc.matches(id, ":has(+ #empty)") && doc.matches(id, ":has(~ main:empty)"));
        assert!(!doc.matches(empty, ":has(+ main)") && !doc.matches(id, ":has(+ code)"));
    }

    #[test]
    fn selectors_follow_element_relationships() {
        let doc = Document::parse(
            "<html><main><section class='middle'><div class='middle'><span id='leaf'></span></div></section><i></i>text<b id='last'></b></main></html>",
        );
        let leaf = doc.nodes.iter().position(|n| n.tag == "span").unwrap();
        let last = doc.nodes.iter().position(|n| n.tag == "b").unwrap();
        assert!(doc.matches(leaf, "main>.middle #leaf"));
        assert!(!doc.matches(leaf, "main>.middle>#leaf"));
        for selector in ["section+i+b", "section~b", "#absent, #last", "*#last"] {
            assert!(doc.matches(last, selector), "{selector}");
        }
        for selector in ["main>>b", "main>", ".", ":unknown", "#last,"] {
            assert!(!doc.matches(last, selector), "{selector}");
        }
        for (id, node) in doc.nodes.iter().enumerate() {
            if node.tag.starts_with('#') {
                assert!(!doc.matches(id, "*"));
            }
        }
        let selector = &Selector::parse("b:not(#last)", None)[0];
        let mut decoded: Selector =
            postcard::from_bytes(&postcard::to_allocvec(selector).unwrap()).unwrap();
        decoded.bind_parent(None).unwrap();
        assert!(!selector.matches(&doc, last) && !decoded.matches(&doc, last));
        assert!(!doc.matches(last, &format!("{}b{}", ":is(".repeat(40), ")".repeat(40))));
    }

    #[test]
    fn raw_text_and_attributes() {
        let doc = Document::parse(
            "<!doctype html><html><body><h1 id='g' data-x=\"a>b\">你好 &amp; &#65;</h1><script>if (a < b) x = '<div>';</script></body></html>",
        );
        let h = doc.nodes.iter().position(|n| n.tag == "h1").unwrap();
        assert_eq!(doc.text(h), "你好 & A");
        assert_eq!(doc.nodes[h].attrs["data-x"], "a>b");
        assert!(doc.matches(h, "body #g"));
        assert!(
            doc.nodes
                .iter()
                .any(|n| n.text == "if (a < b) x = '<div>';")
        );
        assert_eq!(Document::parse(&doc.serialize(0)).text(h), doc.text(h));
    }

    #[test]
    fn svg_keeps_case_and_self_closing_siblings() {
        let doc = Document::parse(
            r#"<svg viewBox="0 0 16 16"><defs><linearGradient id="paint" gradientUnits="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/></svg>"#,
        );
        assert_eq!(doc.nodes[1].attrs["viewBox"], "0 0 16 16");
        assert_eq!(doc.nodes[1].children, vec![2, 4, 5]);
        assert_eq!(doc.nodes[3].tag, "linearGradient");
        assert_eq!(doc.nodes[3].attrs["gradientUnits"], "userSpaceOnUse");
        let source = doc.serialize(1);
        let tree = resvg::usvg::Tree::from_str(&source, &Default::default()).unwrap();
        assert_eq!(tree.root().children().len(), 2);
    }

    fn matched(doc: &Document, selector: &str) -> String {
        (0..doc.nodes.len())
            .filter(|&id| doc.matches(id, selector))
            .filter_map(|id| doc.nodes[id].attrs.get("id").cloned())
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn structural_pseudo_classes_follow_an_plus_b() {
        let doc = Document::parse(
            "<ul><li id=a class=k></li><li id=b></li><li id=c class=k></li><li id=d></li><li id=e class=k></li></ul><div><span id=s1></span><i id=i1></i><span id=s2></span><i id=i2></i><span id=s3></span></div><p><b id=only></b></p>",
        );
        for (selector, expected) in [
            ("li:nth-child(odd)", "a,c,e"),
            ("li:nth-child(EVEN)", "b,d"),
            ("li:nth-child(2n+1)", "a,c,e"),
            ("li:nth-child( 2n + 1 )", "a,c,e"),
            ("li:nth-child(2n- 1)", "a,c,e"),
            ("li:nth-child(-n+3)", "a,b,c"),
            ("li:nth-child(-n/**/+3)", "a,b,c"),
            ("li:nth-child(2n/**/+1)", "a,c,e"),
            ("li:nth-child(n+4)", "d,e"),
            ("li:nth-child(+3)", "c"),
            ("li:nth-child(0n+2)", "b"),
            ("li:nth-child(3n)", "c"),
            ("li:nth-child(-2n+5)", "a,c,e"),
            ("li:nth-child(n)", "a,b,c,d,e"),
            ("li:nth-last-child(2)", "d"),
            ("li:nth-last-child(-n+2)", "d,e"),
            ("li:nth-child(odd of .k)", "a,e"),
            ("li:nth-child(2 of .k)", "c"),
            ("li:nth-last-child(1 of .k)", "e"),
            ("li:nth-child(1 of :not(.k))", "b"),
            ("span:nth-of-type(2)", "s2"),
            ("div > :nth-of-type(odd)", "s1,i1,s3"),
            ("div > :nth-last-of-type(1)", "i2,s3"),
            ("div > :first-of-type", "s1,i1"),
            ("div > :last-of-type", "i2,s3"),
            ("div > :only-of-type", ""),
            ("p > :only-of-type, b:only-child", "only"),
            ("li:first-child, li:last-child", "a,e"),
            ("li:nth-child(n+2):nth-child(-n+3)", "b,c"),
        ] {
            assert_eq!(matched(&doc, selector), expected, "{selector}");
        }
    }

    #[test]
    fn attribute_selectors_handle_flags_quotes_and_escapes() {
        let doc = Document::parse(
            "<div id=x data-k='ABC def' lang='en-US' title='a]b c' class='hover:bg md:w-1/2 123'></div>",
        );
        for selector in [
            "[data-k='abc def' i]",
            "[data-k=\"ABC def\" S]",
            "[ data-k ~= \"def\" ]",
            "[data-k~=DEF i]",
            "[data-k^='ab' i]",
            "[lang|=en]",
            "[lang|='EN' i]",
            "[title=\"a]b c\"]",
            "[title='a\\]b c']",
            "[title=\"a\\5d b c\"]",
            "[|title]",
            "[*|title]",
            "div[id=x]",
            ".hover\\:bg.md\\:w-1\\/2",
            ".\\31 23",
        ] {
            assert!(doc.matches(1, selector), "{selector}");
            let source = Selector::parse(selector, None)[0].source();
            assert!(doc.matches(1, &source), "{selector} -> {source}");
        }
        for selector in [
            "[data-k='abc def']",
            "[data-k='abc def' s]",
            "[data-k$='']",
            "[data-k*='']",
            "[data-k^='']",
            "[data-k~='']",
            "[data-k~='C d']",
            "[lang|=e]",
        ] {
            assert!(!doc.matches(1, selector), "{selector}");
        }
    }

    #[test]
    fn invalid_selectors_reject_the_whole_list() {
        for selector in [
            "li:nth-child(2 n)",
            "li:nth-child(n-)",
            "li:nth-child(+ 2)",
            "li:nth-child(2n+-1)",
            "li:nth-child()",
            "li:nth-child(foo)",
            "li:nth-child(1.5)",
            "li:nth-of-type(1 of .k)",
            "[data-k=1]",
            "[data-k='x' q]",
            "[data-k='x]",
            "[=x]",
            "[a b]",
            "[ns|a]",
            ".1a",
            "#-1",
            ":not()",
            ":has()",
            "a::",
            ":lang()",
            ":dir()",
            "a, :not(.b",
        ] {
            assert!(
                Selector::parse(&format!("p, {selector}"), None).is_empty(),
                "{selector}"
            );
        }
        for selector in [
            ":is()",
            ":is(#1, .a)",
            ":where(.a, 1x)",
            ":unknown",
            "::-webkit-scrollbar",
            ":-webkit-any-link",
        ] {
            assert!(!Selector::parse(selector, None).is_empty(), "{selector}");
        }
        assert!(Selector::parse(&"a ".repeat(2000), None).is_empty());
    }

    #[test]
    fn specificity_follows_selectors_level_4() {
        for (selector, expected) in [
            ("*", [0, 0, 0]),
            ("li", [0, 0, 1]),
            ("li::before", [0, 0, 2]),
            ("li:before", [0, 0, 2]),
            (".a:hover", [0, 2, 0]),
            ("#a .b > c", [1, 1, 1]),
            (":is(#a, .b)", [1, 0, 0]),
            (":where(#a, .b) c", [0, 0, 1]),
            (":not(.a, #b)", [1, 0, 0]),
            (":has(> img, .x)", [0, 1, 0]),
            (":nth-child(2n of #a, .b)", [1, 1, 0]),
            (":nth-child(2n)", [0, 1, 0]),
            ("[data-k='v' i]", [0, 1, 0]),
            (":root", [0, 1, 0]),
            (":host(.a)", [0, 2, 0]),
        ] {
            assert_eq!(
                Selector::parse(selector, None)[0].specificity,
                expected,
                "{selector}"
            );
        }
    }

    #[test]
    fn language_direction_link_and_form_pseudo_classes() {
        let doc = Document::parse(
            "<html lang=en-US><body><div lang=fr><p id=p1></p></div><p id=p2></p><div dir=rtl><span id=r></span><span id=l dir=ltr></span><span id=auto dir=auto></span></div><a id=link href='#x'></a><a id=plain></a><input id=t placeholder=x><input id=v placeholder=x value=1><input id=ro readonly><input id=cb type=checkbox checked required><input id=dis disabled><textarea id=ta required></textarea><div id=ce contenteditable><b id=inner></b></div><progress id=pr></progress><details id=det open></details></body></html>",
        );
        for (selector, expected) in [
            ("p:lang(en)", "p2"),
            ("p:lang(fr)", "p1"),
            ("p:lang(\"EN\", de)", "p2"),
            ("p:lang(en-US)", "p2"),
            ("p:lang(en-u)", ""),
            ("p:lang(*)", "p1,p2"),
            ("span:dir(rtl)", "r"),
            ("span:dir(ltr)", "l,auto"),
            ("a:link", "link"),
            ("a:any-link", "link"),
            ("a:visited, :target", ""),
            ("input:placeholder-shown", "t"),
            ("input:read-only", "ro,cb,dis"),
            ("input:read-write, textarea:read-write", "t,v,ta"),
            ("div:read-write, b:read-write", "ce,inner"),
            ("input:checked", "cb"),
            ("input:required", "cb"),
            ("input:optional", "t,v,ro,dis"),
            (":invalid", "ta"),
            ("input:valid", "t,v,ro,cb,dis"),
            ("input:disabled", "dis"),
            ("input:enabled", "t,v,ro,cb"),
            ("div:enabled, div:disabled", ""),
            ("progress:indeterminate", "pr"),
            ("details:open", "det"),
            ("p:defined", "p1,p2"),
        ] {
            assert_eq!(matched(&doc, selector), expected, "{selector}");
        }
        let html = doc.nodes.iter().position(|n| n.tag == "html").unwrap();
        assert!(Selector::parse(":scope", None)[0].matches(&doc, html));
    }

    #[test]
    fn interaction_states_invalidate_cached_styles() {
        let mut doc = Document::parse(
            "<style>.card:hover{color:red}.card:hover .title{font-weight:700}:focus-within>.title{background-color:blue}.title:focus{color:green}button:active{color:orange}:has(>.title:hover){padding-top:1px}</style><div class=card><span class=title>t</span><button>b</button></div>",
        );
        let card = doc.nodes.iter().position(|n| n.tag == "div").unwrap();
        let title = doc.nodes.iter().position(|n| n.tag == "span").unwrap();
        let button = doc.nodes.iter().position(|n| n.tag == "button").unwrap();
        let checks = [
            (card, "color", "red"),
            (card, "padding-top", "1px"),
            (title, "font-weight", "700"),
            (title, "background-color", "blue"),
            (title, "color", "green"),
            (button, "color", "orange"),
        ];
        let mut sheets = HashMap::new();
        let mut cache = crate::render::StyleCache::default();
        for (step, (states, expected)) in [
            ([0, 0, 0], &[][..]),
            ([HOVER, 0, 0], &[0, 2][..]),
            ([HOVER, HOVER, 0], &[0, 1, 2][..]),
            ([0, 0, 0], &[][..]),
            ([FOCUS_WITHIN, FOCUS | FOCUS_WITHIN, 0], &[3, 4][..]),
            ([HOVER | ACTIVE, 0, HOVER | ACTIVE], &[0, 2, 5][..]),
            ([0, 0, 0], &[][..]),
        ]
        .into_iter()
        .enumerate()
        {
            for (id, state) in [card, title, button].into_iter().zip(states) {
                doc.nodes[id].state = state;
            }
            let cached = crate::render::compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut cache,
                None,
            );
            let fresh = crate::render::compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut crate::render::StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "step {step}");
            for (index, (id, key, value)) in checks.into_iter().enumerate() {
                assert_eq!(
                    cached.values[id].get(key).is_some_and(|v| v == value),
                    expected.contains(&index),
                    "step {step}: {key}:{value}"
                );
            }
        }
    }
}
