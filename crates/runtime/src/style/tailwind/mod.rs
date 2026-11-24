use super::{Declaration, Declarations, Prop, Rule, Style, Stylesheet};
use crate::css::value::color::color;
use crate::dom::Document;
use crate::style::tailwind::tables::{
    BASE, DEFAULTS, INLINE, KEYFRAMES, ORDER_INDEX, PREFLIGHT, PROPERTIES,
};
use crate::style::tailwind::value::{calls, escape, removes};
use rustc_hash::FxHashMap;
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    rc::Rc,
    sync::LazyLock,
};

mod tables;
mod utility;
mod value;
mod variant;

type Pairs = Vec<(String, String)>;
type Nested = (Vec<String>, String, Pairs);

#[derive(Clone, Default)]
struct Body {
    declarations: Pairs,
    nested: Vec<Nested>,
    sort: Option<&'static str>,
}

impl Body {
    fn new(declarations: &[(&str, String)]) -> Option<Self> {
        Some(Self {
            declarations: declarations
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            ..Self::default()
        })
    }

    fn parse(source: &str) -> Self {
        Self {
            declarations: source
                .split(';')
                .filter_map(|item| item.split_once(':'))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            ..Self::default()
        }
    }

    // Tailwind's sort key of a rule: the order of its properties, then more declarations first.
    fn order(&self) -> (Vec<u16>, Reverse<usize>) {
        let mut properties = BTreeSet::new();
        let mut count = 0;
        for (key, _) in self
            .declarations
            .iter()
            .chain(self.nested.iter().flat_map(|(.., values)| values))
        {
            count += 1;
            if let Some(&index) = ORDER_INDEX.get(self.sort.unwrap_or(key)) {
                properties.insert(index);
            }
        }
        let mut properties: Vec<_> = properties.into_iter().collect();
        properties.push(u16::MAX);
        (properties, Reverse(count))
    }

    fn nest(mut self, at: &[&str], selector: &str, source: &str) -> Self {
        let pairs = Self::parse(source).declarations;
        self.nested.push((
            at.iter().map(|&rule| rule.to_owned()).collect(),
            selector.to_owned(),
            pairs,
        ));
        self
    }
}

#[derive(Default)]
struct Variant {
    branches: Vec<(Vec<String>, Option<String>)>,
    content: bool,
    never: bool,
    relative: bool,
}

type Rank = (u16, String);

type Expansion = (usize, Option<String>, Option<String>);

#[derive(Clone, PartialEq, Eq)]
struct Sort {
    variants: Vec<Rank>,
    properties: Vec<u16>,
    count: Reverse<usize>,
    name: String,
}

impl PartialOrd for Sort {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Sort {
    fn cmp(&self, other: &Self) -> Ordering {
        (&self.variants, &self.properties, self.count)
            .cmp(&(&other.variants, &other.properties, other.count))
            .then_with(|| {
                // Tailwind compares digit runs as numbers, so `p-2` sorts before `p-10`.
                let (a, z) = (self.name.as_bytes(), other.name.as_bytes());
                let mut i = 0;
                while i < a.len().min(z.len()) {
                    if a[i].is_ascii_digit() && z[i].is_ascii_digit() {
                        let end =
                            |s: &[u8]| i + s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
                        let (x, y) = (&a[i..end(a)], &z[i..end(z)]);
                        let trim =
                            |s: &[u8]| s.len() - s.iter().take_while(|&&b| b == b'0').count();
                        let order = trim(x)
                            .cmp(&trim(y))
                            .then_with(|| x[x.len() - trim(x)..].cmp(&y[y.len() - trim(y)..]))
                            .then_with(|| x.cmp(y));
                        if order.is_ne() {
                            return order;
                        }
                        i += x.len();
                    } else if a[i] != z[i] {
                        return a[i].cmp(&z[i]);
                    } else {
                        i += 1;
                    }
                }
                a.len().cmp(&z.len())
            })
    }
}

struct Compiled {
    sort: Sort,
    rules: Vec<Nested>,
}

pub(crate) struct Update<'a> {
    pub(crate) root: Rc<Style>,
    pub(crate) replaced: Vec<Rc<Stylesheet>>,
    pub(crate) added: Vec<(Rc<Stylesheet>, Vec<Rc<Stylesheet>>)>,
    pub(crate) rules: Vec<&'a Rule>,
    pub(crate) keys: &'a FxHashMap<usize, u64>,
}

#[derive(Default)]
struct Config {
    key: String,
    lists: Vec<Vec<Rc<Stylesheet>>>,
    theme: BTreeMap<String, String>,
    inline: BTreeSet<String>,
    cleared: Vec<String>,
    variants: Vec<(String, String)>,
    utilities: Vec<(String, String)>,
    expanding: std::cell::RefCell<Vec<usize>>,
    // Do not cache expansions cut short by cycles or depth limits: their result depends on the starting
    // point.
    expanded: std::cell::RefCell<FxHashMap<Expansion, Option<Body>>>,
    cut: std::cell::Cell<bool>,
    import: Option<bool>,
    classes: FxHashMap<String, bool>,
    scanned: Vec<u64>,
    applied: Vec<(Rc<Stylesheet>, Rc<Stylesheet>)>,
    sorted: Vec<(Sort, u64, Vec<Rule>)>,
    chunks: Vec<(u64, Rc<Stylesheet>)>,
    keys: FxHashMap<usize, u64>,
    header: Option<Rc<Stylesheet>>,
    vars: BTreeMap<String, String>,
    properties: BTreeSet<&'static str>,
    keyframes: BTreeSet<&'static str>,
    root: Rc<Style>,
}

#[derive(Default)]
pub(crate) struct Compiler(Vec<Config>);

impl Compiler {
    pub(crate) fn update(
        &mut self,
        sheets: &mut Vec<Rc<Stylesheet>>,
        doc: &Document,
        order: &[(u32, u32)],
    ) -> Option<Update<'_>> {
        let index = sheets.iter().position(|sheet| {
            sheet
                .0
                .iter()
                .any(|rule| matches!(**rule, Rule::Tailwind(_)))
        })?;
        let at = match self.0.iter().position(|config| config.serves(sheets)) {
            Some(at) => at,
            None => {
                let key: String = sheets
                    .iter()
                    .flat_map(|sheet| &sheet.0)
                    .filter(|rule| {
                        matches!(
                            ***rule,
                            Rule::Tailwind(_)
                                | Rule::Theme(..)
                                | Rule::Variant(..)
                                | Rule::Utility(..)
                        )
                    })
                    .map(|rule| rule.source())
                    .collect();
                let at = self
                    .0
                    .iter()
                    .position(|config| config.key == key)
                    .unwrap_or_else(|| {
                        if self.0.len() == 4 {
                            self.0.remove(0);
                        }
                        self.0.push(Config::new(key, sheets));
                        self.0.len() - 1
                    });
                self.0[at].remember(sheets);
                at
            }
        };
        self.0[at..].rotate_left(1);
        let config = self.0.last_mut()?;
        Some(config.update(sheets, index, doc, order))
    }
}

impl Config {
    // Whether this configuration compiled this exact list of sheets lately.
    fn serves(&self, sheets: &[Rc<Stylesheet>]) -> bool {
        self.lists.iter().any(|list| {
            list.len() == sheets.len() && list.iter().zip(sheets).all(|(a, b)| Rc::ptr_eq(a, b))
        })
    }

    fn remember(&mut self, sheets: &[Rc<Stylesheet>]) {
        if self.lists.len() == 4 {
            self.lists.remove(0);
        }
        self.lists.push(sheets.to_vec());
    }

    fn new(key: String, sheets: &[Rc<Stylesheet>]) -> Self {
        let mut config = Self {
            key,
            ..Self::default()
        };
        for rule in sheets.iter().flat_map(|sheet| &sheet.0) {
            match &**rule {
                Rule::Tailwind(Some(preflight)) => {
                    config.import = Some(config.import == Some(true) || *preflight);
                }
                Rule::Theme(inline, children) => {
                    for child in children {
                        let Rule::Declarations(values) = child else {
                            continue;
                        };
                        for value in &values.0 {
                            let Declaration::Property(name, value) = value else {
                                continue;
                            };
                            if name.ends_with("-*") || value == "initial" {
                                config.theme.retain(|other, _| !removes(name, other));
                                config.inline.retain(|other| !removes(name, other));
                                config.cleared.push(name.clone());
                                continue;
                            }
                            if *inline {
                                config.inline.insert(name.clone());
                            } else {
                                config.inline.remove(name);
                            }
                            config.theme.insert(name.clone(), value.clone());
                        }
                    }
                }
                Rule::Variant(name, template) => {
                    config.variants.retain(|(other, _)| other != name);
                    config.variants.push((name.clone(), template.clone()));
                }
                Rule::Utility(name, body) => config.utilities.push((name.clone(), body.clone())),
                _ => {}
            }
        }
        config
    }

    fn update(
        &mut self,
        sheets: &mut Vec<Rc<Stylesheet>>,
        index: usize,
        doc: &Document,
        order: &[(u32, u32)],
    ) -> Update<'_> {
        let mut used = String::new();
        let first = self.header.is_none();
        if first {
            for name in self
                .theme
                .keys()
                .filter(|name| !self.inline.contains(*name))
            {
                let _ = write!(used, "var({name})");
            }
            if self.import == Some(true) {
                used.push_str(PREFLIGHT);
            }
        }
        let mut fresh = Vec::new();
        if self.scanned.len() < doc.nodes.len() {
            self.scanned.resize(doc.nodes.len(), 0);
        }
        for &(id, _) in order {
            let id = id as usize;
            let revision = doc.nodes.revision(id);
            if std::mem::replace(&mut self.scanned[id], revision) == revision {
                continue;
            }
            let node = &doc.nodes[id];
            for (name, value) in node.attrs.iter() {
                if name != "class" {
                    if value.contains("var(--") {
                        used.push_str(value);
                    }
                    continue;
                }
                if self.import.is_none() {
                    continue;
                }
                for class in value.split_ascii_whitespace() {
                    if self.classes.contains_key(class) {
                        continue;
                    }
                    let compiled = self.compile(class, &escape(class));
                    self.classes.insert(class.to_owned(), compiled.is_some());
                    if let Some(compiled) = compiled {
                        let css = css(&compiled.rules);
                        used.push_str(&css);
                        let rules = Stylesheet::parse(&css).0.into_iter();
                        fresh.push((compiled.sort, rules.map(Rc::unwrap_or_clone).collect()));
                    }
                }
            }
        }
        if self.classes.len() > 1 << 16 {
            self.classes.retain(|_, compiled| *compiled);
        }
        for sheet in sheets.iter_mut() {
            if let Some((_, applied)) = self
                .applied
                .iter()
                .find(|(source, _)| Rc::ptr_eq(source, sheet))
            {
                *sheet = applied.clone();
                continue;
            }
            let rules = self.apply(&sheet.0, None, &mut used);
            let applied = rules.map_or_else(
                || sheet.clone(),
                |rules| Rc::new(Stylesheet(rules.into_iter().map(Rc::new).collect())),
            );
            for rule in &applied.0 {
                used.push_str(&rule.source());
            }
            self.applied.push((sheet.clone(), applied.clone()));
            *sheet = applied;
        }
        let lists = &self.lists;
        self.applied.retain(|(source, _)| {
            lists
                .iter()
                .any(|list| list.iter().any(|sheet| Rc::ptr_eq(sheet, source)))
        });
        let header = self.vars(&used) || first;
        let mut replaced = Vec::new();
        let mut added = Vec::new();
        let mut rules = Vec::new();
        if header {
            replaced.extend(self.header.take());
            let mut css = String::from("@layer properties;@layer theme,base,components,utilities;");
            for (name, value) in PROPERTIES {
                if self.properties.contains(name) {
                    let _ = write!(css, "@property {name}{{syntax:\"*\";inherits:false;");
                    if *value != "initial" {
                        let _ = write!(css, "initial-value:{value}");
                    }
                    css.push('}');
                }
            }
            css.push_str("@layer theme{:root,:host{");
            for (name, value) in &self.vars {
                let _ = write!(css, "{name}:{value};");
            }
            css.push_str("}}");
            for (name, frames) in KEYFRAMES {
                if self.keyframes.contains(name) {
                    let _ = write!(css, "@keyframes {name}{{{frames}}}");
                }
            }
            self.header = Some(Rc::new(Stylesheet::parse(&css)));
            self.root = Rc::new(
                self.vars
                    .iter()
                    .map(|(name, value)| (Prop::new(name), value.as_str().into()))
                    .collect(),
            );
        }
        let mut new = BTreeSet::new();
        if !fresh.is_empty() {
            fresh.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            // Keys must stay below 2^56, so a selector index fits below them in 64 bits.
            const KEYS: u64 = 1 << 56;
            let mut renumber = false;
            let mut merged = Vec::with_capacity(self.sorted.len() + fresh.len());
            let mut fresh = fresh.into_iter().peekable();
            let mut sorted = std::mem::take(&mut self.sorted).into_iter().peekable();
            while let Some(next) = fresh.peek() {
                while let Some(old) = sorted.next_if(|old| old.0 < next.0) {
                    merged.push(old);
                }
                let low = merged.last().map_or(0, |(_, key, _)| *key);
                let high = sorted.peek().map_or(KEYS, |(_, key, _)| *key);
                let mut run = Vec::new();
                while let Some(item) =
                    fresh.next_if(|item| sorted.peek().is_none_or(|old| item.0 < old.0))
                {
                    run.push(item);
                }
                let step = (high - low) / (run.len() as u64 + 1);
                renumber |= step == 0;
                for (index, (sort, rules)) in run.into_iter().enumerate() {
                    let key = low + step * (index as u64 + 1);
                    new.insert(key);
                    merged.push((sort, key, rules));
                }
            }
            merged.extend(sorted);
            self.sorted = merged;
            if renumber {
                let step = KEYS / (self.sorted.len() as u64 + 1);
                for (index, (_, key, _)) in self.sorted.iter_mut().enumerate() {
                    *key = step * (index as u64 + 1);
                }
                self.keys.clear();
            }
            for (_, key, rules) in &self.sorted {
                if !renumber && !new.contains(key) {
                    continue;
                }
                let mut index = 0;
                for rule in rules {
                    selectors(rule, &mut |list| {
                        self.keys
                            .insert(Rc::as_ptr(list) as usize, *key << 8 | index);
                        index = (index + 1).min(255);
                    });
                }
            }
            if renumber || self.chunks.is_empty() {
                replaced.extend(self.chunks.drain(..).map(|(_, sheet)| sheet));
                for start in (0..self.sorted.len()).step_by(128) {
                    let end = (start + 128).min(self.sorted.len());
                    let next = self.sorted.get(end).map_or(u64::MAX, |(_, key, _)| *key);
                    self.chunks.push((next, chunk(&self.sorted[start..end])));
                }
            } else {
                let mut index = 0;
                while index < self.chunks.len() {
                    let low = index
                        .checked_sub(1)
                        .map_or(0, |previous| self.chunks[previous].0);
                    let high = self.chunks[index].0;
                    let start = self.sorted.partition_point(|(_, key, _)| *key < low);
                    let end = self.sorted.partition_point(|(_, key, _)| *key < high);
                    let range = &self.sorted[start..end];
                    if new.range(low..).next().is_none_or(|key| *key >= high) {
                        index += 1;
                        continue;
                    }
                    let parts = if range.len() > 256 {
                        let middle = range.len() / 2;
                        vec![
                            (range[middle].1, chunk(&range[..middle])),
                            (high, chunk(&range[middle..])),
                        ]
                    } else {
                        vec![(high, chunk(range))]
                    };
                    let count = parts.len();
                    let sheets = parts.iter().map(|(_, sheet)| sheet.clone()).collect();
                    added.push((self.chunks[index].1.clone(), sheets));
                    self.chunks.splice(index..=index, parts);
                    index += count;
                }
            }
        }
        if replaced.is_empty() {
            rules = new
                .iter()
                .filter_map(|key| {
                    self.sorted
                        .binary_search_by_key(key, |(_, key, _)| *key)
                        .ok()
                })
                .flat_map(|index| &self.sorted[index].2)
                .collect();
        } else {
            replaced.extend(added.drain(..).map(|(old, _)| old));
        }
        if self.import.is_some() {
            let generated = self
                .header
                .iter()
                .chain(self.chunks.iter().map(|(_, sheet)| sheet));
            let base = (self.import == Some(true)).then(|| BASE.with(Clone::clone));
            sheets.splice(index..index, generated.cloned().chain(base));
        }
        Update {
            root: self.root.clone(),
            replaced,
            added,
            rules,
            keys: &self.keys,
        }
    }

    fn vars(&mut self, css: &str) -> bool {
        let mut changed = false;
        let mut pending: Vec<String> = names(css, "var(--")
            .map(|name| format!("--{name}"))
            .collect();
        while let Some(name) = pending.pop() {
            if self.vars.contains_key(&name) || self.inline.contains(&name) {
                continue;
            }
            let value = match self.theme.get(&name) {
                Some(value) => value.clone(),
                None if self.removed(&name) || INLINE.contains(&name.as_str()) => continue,
                None => {
                    let Some(&value) = DEFAULTS.get(name.as_str()) else {
                        continue;
                    };
                    match value
                        .strip_prefix("--theme(")
                        .and_then(|value| value.strip_suffix(", initial)"))
                    {
                        Some(other) if self.inline.contains(other) => self.theme[other].clone(),
                        Some(other) if self.raw(other).is_some() => format!("var({other})"),
                        Some(_) => continue,
                        None => value.to_owned(),
                    }
                }
            };
            pending.extend(names(&value, "var(--").map(|name| format!("--{name}")));
            if let Some(animation) = name.strip_prefix("--animate-")
                && let Some((frames, _)) = KEYFRAMES.iter().find(|(frames, _)| *frames == animation)
            {
                self.keyframes.insert(frames);
            }
            self.vars.insert(name, value);
            changed = true;
        }
        for name in names(css, "--tw-") {
            if let Some((property, _)) = PROPERTIES
                .iter()
                .find(|(property, _)| property[5..] == *name)
            {
                changed |= self.properties.insert(property);
            }
        }
        changed
    }

    fn apply<R: std::borrow::Borrow<Rule>>(
        &self,
        rules: &[R],
        parent: Option<&str>,
        used: &mut String,
    ) -> Option<Vec<Rule>> {
        let mut changed = false;
        let mut result = Vec::with_capacity(rules.len());
        for rule in rules {
            let rule = rule.borrow();
            let rewritten = match rule {
                Rule::Style(selectors, children) => {
                    let base: Vec<_> = selectors
                        .iter()
                        .map(|selector| match parent {
                            Some(parent) => {
                                selector.source().replace('&', &format!(":is({parent})"))
                            }
                            None => selector.source(),
                        })
                        .collect();
                    let base = match base.as_slice() {
                        [single] => single.clone(),
                        _ => format!(":is({})", base.join(",")),
                    };
                    self.apply(children, Some(&base), used)
                        .map(|children| vec![Rule::Style(selectors.clone(), children)])
                }
                Rule::Declarations(values)
                    if values.0.iter().any(|value| match value {
                        Declaration::Apply(_) => true,
                        Declaration::Property(_, value) => calls(value),
                    }) =>
                {
                    let mut declarations = Vec::new();
                    let mut extra = String::new();
                    for value in &values.0 {
                        match value {
                            Declaration::Property(name, value) => declarations
                                .push(Declaration::Property(name.clone(), self.functions(value))),
                            Declaration::Apply(classes) => {
                                let Some(base) = parent else {
                                    continue;
                                };
                                let mut compiled: Vec<_> = classes
                                    .iter()
                                    .filter_map(|class| self.compile(class, base))
                                    .collect();
                                compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                                for (at, selector, pairs) in
                                    compiled.into_iter().flat_map(|c| c.rules)
                                {
                                    if at.is_empty() && selector == base {
                                        for (name, value) in pairs {
                                            used.push_str(&value);
                                            declarations.push(Declaration::Property(name, value));
                                        }
                                    } else {
                                        extra.push_str(&css(&[(at, selector, pairs)]));
                                    }
                                }
                            }
                        }
                    }
                    used.push_str(&extra);
                    let mut rules = vec![Rule::Declarations(Declarations(declarations))];
                    rules.extend(
                        Stylesheet::parse(&extra)
                            .0
                            .into_iter()
                            .map(Rc::unwrap_or_clone),
                    );
                    Some(rules)
                }
                Rule::Variant(name, body) if parent.is_some() && body.starts_with('{') => {
                    let base = parent.unwrap_or_default();
                    let body = body
                        .strip_prefix('{')
                        .and_then(|body| body.strip_suffix('}'))
                        .unwrap_or_default();
                    let mut css = String::new();
                    let branches = self.variant(name, 0).map(|(_, variant)| variant.branches);
                    for (at, selector) in branches.into_iter().flatten() {
                        let selector =
                            selector.map_or_else(|| base.to_owned(), |t| t.replace('&', base));
                        for rule in &at {
                            let _ = write!(css, "{rule}{{");
                        }
                        let _ = write!(css, "{selector}{{{body}}}");
                        css.push_str(&"}".repeat(at.len()));
                    }
                    let parsed: Vec<_> = Stylesheet::parse(&css)
                        .0
                        .into_iter()
                        .map(Rc::unwrap_or_clone)
                        .collect();
                    Some(self.apply(&parsed, None, used).unwrap_or(parsed))
                }
                Rule::Media(queries, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Media(queries.clone(), children)]),
                Rule::Layer(names, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Layer(names.clone(), children)]),
                Rule::Supports(condition, supported, children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Supports(condition.clone(), *supported, children)]),
                Rule::Container(name, block, query, children) => {
                    self.apply(children, parent, used).map(|children| {
                        vec![Rule::Container(
                            name.clone(),
                            *block,
                            query.clone(),
                            children,
                        )]
                    })
                }
                _ => None,
            };
            match rewritten {
                Some(rules) => {
                    changed = true;
                    result.extend(rules);
                }
                None => result.push(rule.clone()),
            }
        }
        changed.then_some(result)
    }
}

fn css(rules: &[Nested]) -> String {
    let mut css = String::new();
    for (at, selector, pairs) in rules {
        for rule in at {
            let _ = write!(css, "{rule}{{");
        }
        let _ = write!(css, "{selector}{{");
        for (key, value) in pairs {
            let _ = write!(css, "{key}:{value};");
        }
        css.push_str(&"}".repeat(at.len() + 1));
    }
    css
}

fn chunk(utilities: &[(Sort, u64, Vec<Rule>)]) -> Rc<Stylesheet> {
    let rules = utilities
        .iter()
        .flat_map(|(.., rules)| rules.iter().cloned())
        .collect();
    Rc::new(Stylesheet(vec![Rc::new(Rule::Layer(
        vec!["utilities".into()],
        rules,
    ))]))
}

fn selectors(rule: &Rule, visit: &mut impl FnMut(&super::SelectorList)) {
    match rule {
        Rule::Style(list, children) => {
            visit(list);
            for child in children {
                selectors(child, visit);
            }
        }
        Rule::Media(_, children)
        | Rule::Layer(_, children)
        | Rule::Supports(_, _, children)
        | Rule::Container(.., children) => {
            for child in children {
                selectors(child, visit);
            }
        }
        _ => {}
    }
}

fn names<'a>(text: &'a str, prefix: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(prefix).filter_map(move |(index, _)| {
        let rest = &text[index + prefix.len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    })
}

#[cfg(test)]
mod tests;

// Compiles Tailwind for a page that a system web view shows, like the developer tools: `css`, and the
// classes among the words of `text`.
pub(crate) fn page_css(css: &str, text: &str) -> String {
    let stop = |c: char| c.is_whitespace() || "\"'`<>=;,&".contains(c);
    let words: Vec<_> = text.split(stop).filter(|word| !word.is_empty()).collect();
    let doc = Document::parse(&format!("<p class=\"{}\">", words.join(" ")));
    let order: Vec<_> = (0..doc.nodes.len() as u32).map(|id| (id, 0)).collect();
    let mut sheets = vec![Rc::new(Stylesheet::parse(css))];
    Compiler::default().update(&mut sheets, &doc, &order);
    // A web view does not know the rules that only configure Tailwind.
    let plain = |rule: &&Rc<Rule>| {
        !matches!(
            ***rule,
            Rule::Tailwind(_) | Rule::Theme(..) | Rule::Variant(..) | Rule::Utility(..)
        )
    };
    let rules = sheets.iter().flat_map(|sheet| &sheet.0).filter(plain);
    rules.map(|rule| rule.source()).collect()
}
