use bbtree::unsync::BBTreeMap;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, rc::Rc};

pub(crate) type SelectorList = Rc<Vec<Selector>>;

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
    Attribute(String, String, String),
    Pseudo(String, String, #[serde(skip)] Vec<Selector>),
    Parent(#[serde(skip)] SelectorList, usize),
}

fn selector_boundary(source: &str, separator: impl Fn(char) -> bool) -> usize {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, c) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
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
    nodes: Vec<(Node, [u64; 5])>,
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
            .resize_with(doc.nodes.len(), || (Node::default(), [0; 5]));
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
            ];
            if changed.iter().any(|&value| value) {
                for (kind, (revision, changed)) in revisions.iter_mut().zip(changed).enumerate() {
                    if changed {
                        *revision += 1;
                        changes.push(id * 5 + kind);
                    }
                }
                *old = Node {
                    tag: node.tag.clone(),
                    attrs: node.attrs.clone(),
                    inline: node.inline.clone(),
                    parent: node.parent,
                    children: node.children.clone(),
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
            self.dependencies.push((id * 5 + kind, revisions[kind]));
        }
    }

    pub(crate) fn valid(&self, dependencies: &[(usize, u64)]) -> bool {
        dependencies.iter().all(|(id, revision)| {
            self.nodes
                .get(*id / 5)
                .map_or(0, |(_, current)| current[*id % 5])
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
    fn relative(argument: &str) -> Vec<Self> {
        let mut parts = Vec::new();
        let mut rest = argument;
        loop {
            let end = selector_boundary(rest, |c| c == ',');
            parts.push(format!(":scope {}", rest[..end].trim()));
            if end == rest.len() {
                break;
            }
            rest = &rest[end + 1..];
        }
        Self::parse(&parts.join(","), None)
    }

    pub(crate) fn bind_parent(&mut self, parent: Option<&SelectorList>) -> anyhow::Result<()> {
        for selector in &mut self.simple {
            match selector {
                SimpleSelector::Parent(selectors, _) => {
                    *selectors = parent
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing CSS parent selector"))?;
                }
                SimpleSelector::Pseudo(name, argument, parsed)
                    if matches!(name.as_str(), "not" | "is" | "where" | "has") =>
                {
                    *parsed = if name == "has" {
                        Self::relative(argument)
                    } else {
                        Self::parse(argument, None)
                    };
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
        let index = self.simple.iter().position(|part| matches!(part, SimpleSelector::Pseudo(name, _, _) if matches!(name.as_str(), "before" | "after" | "selection")))?;
        let mut selector = self.clone();
        let SimpleSelector::Pseudo(name, _, _) = selector.simple.remove(index) else {
            unreachable!()
        };
        Some((
            match name.as_str() {
                "before" => 0,
                "after" => 1,
                _ => 2,
            },
            selector,
        ))
    }

    pub(crate) fn key(&self) -> Option<(u8, &str)> {
        self.simple
            .iter()
            .filter_map(|selector| match selector {
                SimpleSelector::Id(id) => Some((0, id.as_str())),
                SimpleSelector::Class(class) => Some((1, class.as_str())),
                SimpleSelector::Tag(tag) => Some((2, tag.as_str())),
                SimpleSelector::Attribute(name, _, _) => Some((3, name.as_str())),
                _ => None,
            })
            .min_by_key(|(kind, _)| *kind)
            .or_else(|| {
                self.simple.iter().find_map(|part| {
                    let alternatives = match part {
                        SimpleSelector::Parent(selectors, _) => selectors.as_slice(),
                        SimpleSelector::Pseudo(name, _, selectors)
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
        let mut source = String::new();
        if let Some((combinator, previous)) = &self.previous {
            source.push_str(&previous.source());
            source.push(*combinator);
        }
        for selector in &self.simple {
            match selector {
                SimpleSelector::Tag(tag) => source.push_str(tag),
                SimpleSelector::Id(id) => {
                    source.push('#');
                    source.push_str(id);
                }
                SimpleSelector::Class(class) => {
                    source.push('.');
                    source.push_str(class);
                }
                SimpleSelector::Universal => source.push('*'),
                SimpleSelector::Root => source.push_str(":root"),
                SimpleSelector::Attribute(name, op, value) => {
                    source.push('[');
                    source.push_str(name);
                    source.push_str(op);
                    if !op.is_empty() {
                        source.push('"');
                        source.push_str(value);
                        source.push('"');
                    }
                    source.push(']');
                }
                SimpleSelector::Pseudo(name, argument, _) => {
                    source.push(':');
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
        let mut sources = Vec::new();
        let mut remaining = source;
        loop {
            let end = selector_boundary(remaining, |c| c == ',');
            sources.push(&remaining[..end]);
            if end == remaining.len() {
                break;
            }
            remaining = &remaining[end + 1..];
        }
        for source in sources {
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
            while !rest.is_empty() {
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
                        let attribute = &compound[1..end];
                        let (name, operator, value) = if let Some(equal) = attribute.find('=') {
                            let prefix = &attribute[..equal];
                            let (name, operator) = if prefix.ends_with(['~', '|', '^', '$', '*']) {
                                (&prefix[..prefix.len() - 1], &attribute[equal - 1..=equal])
                            } else {
                                (prefix, "=")
                            };
                            (
                                name.trim(),
                                operator,
                                attribute[equal + 1..].trim().trim_matches(['\'', '"']),
                            )
                        } else {
                            (attribute.trim(), "", "")
                        };
                        if name.is_empty() {
                            return Vec::new();
                        }
                        simple.push(SimpleSelector::Attribute(
                            name.into(),
                            operator.into(),
                            value.into(),
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
                    let named = matches!(marker, b'.' | b'#' | b':');
                    if named {
                        compound = &compound[1..];
                        if marker == b':' && compound.starts_with(':') {
                            compound = &compound[1..];
                        }
                    } else if !simple.is_empty() {
                        return Vec::new();
                    }
                    let end = compound
                        .find(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_'))
                        .unwrap_or(compound.len());
                    if end == 0 {
                        return Vec::new();
                    }
                    let name = &compound[..end];
                    simple.push(match marker {
                        b'#' => {
                            specificity[0] = specificity[0].saturating_add(1);
                            SimpleSelector::Id(name.into())
                        }
                        b'.' => {
                            specificity[1] = specificity[1].saturating_add(1);
                            SimpleSelector::Class(name.into())
                        }
                        b':' if name == "root" => {
                            specificity[1] = specificity[1].saturating_add(1);
                            SimpleSelector::Root
                        }
                        b':' => {
                            specificity[1] = specificity[1].saturating_add(1);
                            SimpleSelector::Pseudo(name.into(), String::new(), Vec::new())
                        }
                        _ => {
                            specificity[2] = specificity[2].saturating_add(1);
                            SimpleSelector::Tag(name.into())
                        }
                    });
                    compound = &compound[end..];
                    if marker == b':' && compound.starts_with('(') {
                        let close = 1 + selector_boundary(&compound[1..], |c| c == ')');
                        if close == compound.len() {
                            return Vec::new();
                        }
                        if let Some(SimpleSelector::Pseudo(name, argument, parsed)) =
                            simple.last_mut()
                        {
                            *argument = compound[1..close].into();
                            if matches!(name.as_str(), "not" | "is" | "where" | "has") {
                                *parsed = if name == "has" {
                                    Self::relative(argument)
                                } else {
                                    Self::parse(argument, None)
                                };
                            }
                        }
                        compound = &compound[close + 1..];
                    }
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

    fn matches_cached(
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
    ) -> bool {
        let key = (self as *const Self as usize, id, scope, false);
        if let Some((result, dependencies)) = memo.values.get(&key)
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
                    SimpleSelector::Attribute(name, operator, value) => node
                        .attrs
                        .get(name)
                        .is_some_and(|actual| match operator.as_str() {
                            "" => true,
                            "=" => actual == value,
                            "~=" => actual.split_whitespace().any(|word| word == value),
                            "|=" => actual
                                .strip_prefix(value.as_str())
                                .is_some_and(|rest| rest.is_empty() || rest.starts_with('-')),
                            "^=" => actual.starts_with(value),
                            "$=" => actual.ends_with(value),
                            "*=" => actual.contains(value),
                            _ => false,
                        }),
                    SimpleSelector::Pseudo(name, argument, parsed) => match name.as_str() {
                        "scope" => scope == Some(id),
                        "has" => {
                            memo.depend(id, 2);
                            let mut pending = node.children.clone();
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
                        "host" => node.tag == "apps-shadow-root",
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
                        "checked" | "disabled" | "required" => node.attrs.contains_key(name),
                        "enabled" => !node.attrs.contains_key("disabled"),
                        "focus" | "focus-visible" => node.attrs.contains_key("data-apps-focus"),
                        "not" => !parsed
                            .iter()
                            .any(|s| s.matches_cached(doc, id, scope, memo)),
                        "is" | "where" => parsed
                            .iter()
                            .any(|s| s.matches_cached(doc, id, scope, memo)),
                        "first-child" | "last-child" | "only-child" | "nth-child" => {
                            memo.depend(id, 1);
                            node.parent.is_some_and(|parent| {
                                memo.depend(parent, 2);
                                for &child in &doc.nodes[parent].children {
                                    memo.depend(child, 4);
                                }
                                let mut elements = doc.nodes[parent]
                                    .children
                                    .iter()
                                    .filter(|&&id| !doc.nodes[id].tag.starts_with('#'));
                                match name.as_str() {
                                    "first-child" => elements.next() == Some(&id),
                                    "last-child" => elements.next_back() == Some(&id),
                                    "only-child" => elements.nth(1).is_none(),
                                    _ => elements.position(|&child| child == id).is_some_and(
                                        |index| match argument.as_str() {
                                            "odd" => index % 2 == 0,
                                            "even" => index % 2 == 1,
                                            _ => argument
                                                .parse::<usize>()
                                                .is_ok_and(|n| n == index + 1),
                                        },
                                    ),
                                }
                            })
                        }
                        _ => false,
                    },
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
                    node.attrs.insert(name, decode(value));
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
}
