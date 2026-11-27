use super::{Declaration, Declarations, Prop, Rule, Style, Stylesheet};
use crate::css::value::color::color;
use crate::dom::Document;
use crate::style::tailwind::tables::{BASE, DEFAULTS, KEYFRAMES, ORDER_INDEX, TEXT};
use crate::style::tailwind::value::{braces, calls, escape, escape_name, removes, split};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    rc::Rc,
    sync::LazyLock,
};

mod tables;
mod text;
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

    // Builds the body of a template like `>--tw-sort:row-gap;margin:$`. It starts with `>` for a rule of the
    // children and with `:` for `::placeholder`, and `$` stands for the value.
    fn fill(template: &str, value: &str) -> Self {
        let (selector, template) = match template.as_bytes().first() {
            Some(b'>') => (Some(":where(& > :not(:last-child))"), &template[1..]),
            Some(b':') => (Some("&::placeholder"), &template[1..]),
            _ => (None, template),
        };
        let mut body = Self::default();
        for (key, item) in template.split(';').filter_map(|item| item.split_once(':')) {
            if key == "--tw-sort" {
                body.sort = ORDER_INDEX.get_key_value(item).map(|(&name, _)| name);
                continue;
            }
            body.declarations.push((
                key.to_owned(),
                match item {
                    "$" => value.to_owned(),
                    _ if item.contains('$') => item.replace('$', value),
                    _ => item.to_owned(),
                },
            ));
        }
        if let Some(selector) = selector {
            let pairs = std::mem::take(&mut body.declarations);
            body.nested.push((Vec::new(), selector.to_owned(), pairs));
        }
        body
    }

    // Like Tailwind's rules for one class, the declarations of `other` come after these.
    fn merge(mut self, other: Self) -> Self {
        self.declarations
            .retain(|(key, _)| other.declarations.iter().all(|(name, _)| name != key));
        self.declarations.extend(other.declarations);
        self.nested.extend(other.nested);
        self
    }

    fn push(&mut self, at: Vec<String>, selector: String, pairs: Pairs) {
        if pairs.is_empty() {
            return;
        }
        match self.nested.last_mut() {
            None if at.is_empty() && selector == "&" => self.declarations.extend(pairs),
            Some(last) if last.0 == at && last.1 == selector => last.2.extend(pairs),
            _ => self.nested.push((at, selector, pairs)),
        }
    }

    // Tailwind's sort key of a rule: the order of its properties, then more declarations first.
    fn order(&self) -> (Vec<u16>, Reverse<usize>) {
        let mut properties = BTreeSet::new();
        let mut count = usize::from(self.sort.is_some());
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
        let pairs = Self::fill(source, "").declarations;
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

const INLINE_VALUE: u8 = 1;
const REFERENCE: u8 = 2;
const DEFAULT: u8 = 4;
const STATIC_VALUE: u8 = 8;

#[derive(Clone)]
struct Theme {
    value: String,
    options: u8,
}

// The parts of Tailwind that a stylesheet can import.
const THEME: usize = 0;
const BASE_STYLES: usize = 1;
const UTILITIES: usize = 2;

#[derive(Default)]
struct Config {
    key: String,
    lists: Vec<Vec<Rc<Stylesheet>>>,
    theme: BTreeMap<String, Theme>,
    prefix: String,
    important: bool,
    // The options of the default theme.
    options: u8,
    // The layers of the imported parts of Tailwind.
    imports: [Option<String>; 3],
    referenced: bool,
    excluded: BTreeSet<String>,
    cleared: Vec<String>,
    variants: Vec<(String, String)>,
    utilities: Vec<(String, String)>,
    expanding: std::cell::RefCell<Vec<usize>>,
    // Do not cache expansions cut short by cycles or depth limits: their result depends on the starting
    // point.
    expanded: std::cell::RefCell<FxHashMap<Expansion, Option<Body>>>,
    cut: std::cell::Cell<bool>,
    // The variants of `@custom-variant` that are open, and the bytes made for one class or rule.
    open: std::cell::RefCell<Vec<usize>>,
    work: std::cell::Cell<usize>,
    nested: std::cell::Cell<u8>,
    classes: FxHashMap<String, bool>,
    scanned: Vec<u64>,
    // The attribute values with variables that `vars` has read.
    read: FxHashSet<Box<str>>,
    applied: Vec<(Rc<Stylesheet>, Rc<Stylesheet>)>,
    sorted: Vec<(Sort, u64, Vec<Rule>)>,
    chunks: Vec<(u64, Rc<Stylesheet>)>,
    keys: FxHashMap<usize, u64>,
    header: Option<Rc<Stylesheet>>,
    vars: BTreeMap<String, String>,
    properties: BTreeSet<&'static str>,
    base: Option<Rc<Stylesheet>>,
    order: String,
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
        let index = sheets
            .iter()
            .position(|sheet| sheet.0.iter().any(|rule| imports(rule)))?;
        let at = match self.0.iter().position(|config| config.serves(sheets)) {
            Some(at) => at,
            None => {
                let key: String = sheets
                    .iter()
                    .flat_map(|sheet| &sheet.0)
                    .filter(|rule| configures(rule))
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
            config.configure(rule, None);
        }
        config
    }

    fn configure(&mut self, rule: &Rule, layer: Option<&str>) {
        match rule {
            Rule::Layer(names, children) => {
                for child in children {
                    self.configure(child, names.first().map(String::as_str).or(Some("")));
                }
            }
            Rule::Tailwind(text) => self.import(text, layer),
            Rule::Theme(options, children) => {
                let options = self.options(options);
                for child in children {
                    let Rule::Declarations(values) = child else {
                        continue;
                    };
                    for value in &values.0 {
                        let Declaration::Property(name, value) = value else {
                            continue;
                        };
                        let old = self.theme.get(name);
                        if name.ends_with("-*") {
                            self.theme.retain(|other, _| !removes(name, other));
                            self.cleared.push(name.clone());
                        } else if old.is_some_and(|old| {
                            options & DEFAULT != 0 && old.options & DEFAULT == 0
                                || options & REFERENCE != 0
                                    && old.options & REFERENCE == 0
                                    && old.value == *value
                        }) {
                        } else if value == "initial" {
                            self.theme.remove(name);
                            self.cleared.push(name.clone());
                        } else {
                            let value = value.clone();
                            self.theme.insert(name.clone(), Theme { value, options });
                        }
                    }
                }
            }
            Rule::CustomVariant(name, template) => {
                match self.variants.iter_mut().find(|(other, _)| other == name) {
                    Some((_, old)) => old.clone_from(template),
                    None => self.variants.push((name.clone(), template.clone())),
                }
            }
            Rule::Utility(name, body) => {
                let start = name.strip_prefix('-').unwrap_or(name);
                let utility = (name.clone(), body.clone());
                if start.starts_with(|c: char| c.is_ascii_lowercase())
                    && !self.utilities.contains(&utility)
                {
                    self.utilities.push(utility);
                }
            }
            _ => {}
        }
    }

    // Reads an import, like `import "tailwindcss" prefix(tw) important`.
    fn import(&mut self, text: &str, outer: Option<&str>) {
        let words: Vec<_> = split(text, ' ')
            .into_iter()
            .filter(|word| !word.is_empty())
            .collect();
        let name = words.get(1).copied().unwrap_or_default();
        let name = name.trim_matches(['"', '\'']);
        match words.first().copied() {
            Some("source") => {
                let list = name
                    .eq("not")
                    .then(|| words.get(2)?.strip_prefix("inline(")?.strip_suffix(')'))
                    .flatten()
                    .unwrap_or_default();
                let list = list.trim().trim_matches(['"', '\'']);
                for word in list.split_ascii_whitespace() {
                    self.excluded.extend(braces(word));
                }
                return;
            }
            Some("reference") => {
                self.referenced = true;
                return;
            }
            Some("tailwind") => {
                self.imports[UTILITIES].get_or_insert(outer.unwrap_or_default().to_owned());
                return;
            }
            _ => {}
        }
        let mut layer = outer.map(str::to_owned);
        for word in words.iter().skip(2) {
            if let Some(name) = word
                .strip_prefix("layer(")
                .and_then(|w| w.strip_suffix(')'))
            {
                layer = Some(match layer {
                    Some(inner) => format!("{name}.{inner}"),
                    None => name.to_owned(),
                });
            } else if let Some(options) = word.strip_prefix("theme(") {
                self.options |= self.options(options.strip_suffix(')').unwrap_or(options));
            } else if *word == "important" {
                self.important = true;
            } else if word.starts_with("prefix(") {
                self.options(word);
            }
        }
        let nested = |part: &str| match &layer {
            Some(layer) => format!("{layer}.{part}"),
            None => part.to_owned(),
        };
        match name.trim_end_matches(".css") {
            "tailwindcss/theme" => self.imports[THEME] = Some(layer.unwrap_or_default()),
            "tailwindcss/preflight" => self.imports[BASE_STYLES] = Some(layer.unwrap_or_default()),
            "tailwindcss/utilities" => self.imports[UTILITIES] = Some(layer.unwrap_or_default()),
            _ => {
                self.imports = ["theme", "base", "utilities"].map(|part| Some(nested(part)));
            }
        }
    }

    fn options(&mut self, text: &str) -> u8 {
        let mut options = 0;
        for word in text.split_ascii_whitespace() {
            match word {
                "inline" => options |= INLINE_VALUE,
                "reference" => options |= REFERENCE,
                "default" => options |= DEFAULT,
                "static" => options |= STATIC_VALUE,
                _ => {
                    if let Some(prefix) = word.strip_prefix("prefix(") {
                        let prefix = prefix.trim_end_matches(')');
                        if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_lowercase()) {
                            self.prefix = prefix.to_owned();
                        }
                    }
                }
            }
        }
        options
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
            let statics = (self.options & STATIC_VALUE != 0).then(|| DEFAULTS.keys());
            let names = self.theme.keys().map(String::as_str);
            for name in names.chain(statics.into_iter().flatten().copied()) {
                let _ = write!(used, "var({})", self.prefixed(name));
            }
            if self.imports[BASE_STYLES].is_some() {
                let preflight = match self.prefix.as_str() {
                    "" => TEXT.preflight.to_owned(),
                    _ => TEXT
                        .preflight
                        .replace("var(--default", &format!("var(--{}-default", self.prefix)),
                };
                used.push_str(&preflight);
                let layer = self.imports[BASE_STYLES].as_deref().unwrap_or_default();
                self.base = Some(match (layer, self.prefix.as_str()) {
                    ("base", "") => BASE.with(Clone::clone),
                    ("", _) => Rc::new(Stylesheet::parse(&preflight)),
                    _ => Rc::new(Stylesheet::parse(&format!("@layer {layer}{{{preflight}}}"))),
                });
            }
            // Author layers that are named before the import stay in front.
            let builtin = |sheet: &&Rc<Stylesheet>| {
                #[cfg(target_os = "macos")]
                return crate::style::ua::GLASS_SHEET.with(|glass| Rc::ptr_eq(glass, sheet));
                #[cfg(not(target_os = "macos"))]
                return {
                    let _ = sheet;
                    false
                };
            };
            let sheets = sheets
                .iter()
                .take(index + 1)
                .filter(|sheet| !builtin(sheet));
            for rule in sheets.flat_map(|sheet| &sheet.0) {
                if imports(rule) {
                    break;
                }
                if matches!(&**rule, Rule::Layer(_, children) if children.is_empty()) {
                    self.order.push_str(&rule.source());
                }
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
                    if value.contains("var(--") && !self.read.contains(value.as_str()) {
                        if self.read.len() > 4096 {
                            self.read.clear();
                        }
                        self.read.insert(value.as_str().into());
                        used.push_str(value);
                    }
                    continue;
                }
                if self.imports[UTILITIES].is_none() {
                    continue;
                }
                for class in value.split_ascii_whitespace() {
                    if self.classes.contains_key(class) {
                        continue;
                    }
                    self.work.set(0);
                    let compiled = self.compile(class, &escape(class), self.important);
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
            let mut css = format!(
                "@layer properties;{}@layer theme,base,components,utilities;",
                self.order
            );
            for line in TEXT.properties.lines().skip(1) {
                let mut fields = line.split('|');
                let mut field = || fields.next().unwrap_or_default();
                let (name, value, syntax) = (field(), field(), field());
                if self.properties.contains(&name[5..]) {
                    let _ = write!(css, "@property {name}{{syntax:\"{syntax}\";inherits:false;");
                    if value != "initial" {
                        let _ = write!(css, "initial-value:{value}");
                    }
                    css.push('}');
                }
            }
            let layer = self.imports[THEME].as_deref().unwrap_or("theme");
            if !layer.is_empty() {
                let _ = write!(css, "@layer {layer}{{");
            }
            css.push_str(":root,:host{");
            for (name, value) in &self.vars {
                let _ = write!(css, "{}:{value};", escape_name(name));
            }
            css.push_str(if layer.is_empty() { "}" } else { "}}" });
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
            let layer = self.imports[UTILITIES].as_deref().unwrap_or_default();
            if renumber || self.chunks.is_empty() {
                replaced.extend(self.chunks.drain(..).map(|(_, sheet)| sheet));
                for start in (0..self.sorted.len()).step_by(128) {
                    let end = (start + 128).min(self.sorted.len());
                    let next = self.sorted.get(end).map_or(u64::MAX, |(_, key, _)| *key);
                    self.chunks
                        .push((next, chunk(&self.sorted[start..end], layer)));
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
                            (range[middle].1, chunk(&range[..middle], layer)),
                            (high, chunk(&range[middle..], layer)),
                        ]
                    } else {
                        vec![(high, chunk(range, layer))]
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
        if self.imports.iter().any(Option::is_some) {
            let generated = self
                .header
                .iter()
                .chain(self.chunks.iter().map(|(_, sheet)| sheet));
            let base = self.base.clone();
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

    // Counts the bytes that a class or an author rule makes. Lists of variants and chains of `@apply` can
    // multiply at every level.
    fn spend(&self, bytes: usize) -> Option<()> {
        self.work.set(self.work.get() + bytes + 16);
        if self.work.get() > 1 << 20 {
            self.cut.set(true);
            return None;
        }
        Some(())
    }

    fn vars(&mut self, css: &str) -> bool {
        let mut changed = false;
        let mut pending: Vec<String> = names(css, "var(--")
            .filter_map(|name| self.unprefixed(name))
            .collect();
        while let Some(name) = pending.pop() {
            if self.vars.contains_key(&self.prefixed(&name)) {
                continue;
            }
            let value = match self.theme.get(&name) {
                Some(theme) if theme.options & REFERENCE != 0 => continue,
                Some(theme) => self.functions(&theme.value),
                None if self.raw(&name).is_none() || self.flags(&name) & REFERENCE != 0 => {
                    continue;
                }
                None => {
                    let value = DEFAULTS[name.as_str()];
                    match value
                        .strip_prefix("--theme(")
                        .and_then(|value| value.strip_suffix(", initial)"))
                    {
                        Some(other) => match self.variable(other) {
                            Some(value) => value,
                            None => continue,
                        },
                        None => value.to_owned(),
                    }
                }
            };
            pending.extend(names(&value, "var(--").filter_map(|name| self.unprefixed(name)));
            if let Some(animation) = name.strip_prefix("--animate-")
                && let Some((frames, _)) = KEYFRAMES.iter().find(|(frames, _)| *frames == animation)
            {
                self.keyframes.insert(frames);
            }
            self.vars.insert(self.prefixed(&name), value);
            changed = true;
        }
        for name in names(css, "--tw-") {
            if self.properties.contains(name) {
                continue;
            }
            if let Some(start) = TEXT.properties.find(&format!("\n--tw-{name}|")) {
                let start = start + 6;
                changed |= self
                    .properties
                    .insert(&TEXT.properties[start..start + name.len()]);
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
            if parent.is_none() && self.nested.get() == 0 {
                self.work.set(0);
            }
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
                    let mut rules = Vec::new();
                    let declare = |rules: &mut Vec<Rule>, name: &str, value: String| {
                        let declaration = Declaration::Property(name.to_owned(), value);
                        match rules.last_mut() {
                            Some(Rule::Declarations(values)) => values.0.push(declaration),
                            _ => rules.push(Rule::Declarations(Declarations(vec![declaration]))),
                        }
                    };
                    for value in &values.0 {
                        let classes = match value {
                            Declaration::Property(name, value) => {
                                declare(&mut rules, name, self.functions(value));
                                continue;
                            }
                            Declaration::Apply(classes) => classes,
                        };
                        let Some(base) = parent else {
                            continue;
                        };
                        let mut compiled: Vec<_> = classes
                            .iter()
                            .filter_map(|class| self.compile(class, base, false))
                            .collect();
                        compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                        for (at, selector, pairs) in compiled.into_iter().flat_map(|c| c.rules) {
                            if at.is_empty() && selector == base {
                                for (name, value) in pairs {
                                    used.push_str(&value);
                                    declare(&mut rules, &name, value);
                                }
                                continue;
                            }
                            let extra = css(&[(at, selector, pairs)]);
                            used.push_str(&extra);
                            let parsed = Stylesheet::parse(&extra).0;
                            rules.extend(parsed.into_iter().map(Rc::unwrap_or_clone));
                        }
                    }
                    Some(rules)
                }
                Rule::VariantBlock(name, body) => {
                    let depth = self.nested.get();
                    let base = parent.unwrap_or(":scope");
                    let mut css = String::new();
                    let list = match depth < 32 {
                        true => self.variant_list(name, base, 0),
                        false => None,
                    };
                    for (at, selectors) in list.into_iter().flatten() {
                        if self.spend(body.len()).is_none() {
                            break;
                        }
                        for rule in &at {
                            let _ = write!(css, "{rule}{{");
                        }
                        let selector = selectors.join(",");
                        if parent.is_none() && selector == ":scope" {
                            css.push_str(body);
                        } else {
                            let _ = write!(css, "{selector}{{{body}}}");
                        }
                        css.push_str(&"}".repeat(at.len()));
                    }
                    let parsed: Vec<_> = Stylesheet::parse(&css)
                        .0
                        .into_iter()
                        .map(Rc::unwrap_or_clone)
                        .collect();
                    self.nested.set(depth + 1);
                    let applied = self.apply(&parsed, None, used).unwrap_or(parsed);
                    self.nested.set(depth);
                    Some(applied)
                }
                Rule::Media(queries, children)
                    if queries.iter().any(|query| calls(&query.source())) =>
                {
                    let text: Vec<_> = queries
                        .iter()
                        .map(|query| self.functions(&query.source()))
                        .collect();
                    let parsed = Stylesheet::parse(&format!("@media {}{{}}", text.join(",")));
                    let queries = match parsed.0.first().map(|rule| &**rule) {
                        Some(Rule::Media(queries, _)) => queries.clone(),
                        _ => queries.clone(),
                    };
                    let children = self
                        .apply(children, parent, used)
                        .unwrap_or_else(|| children.clone());
                    Some(vec![Rule::Media(queries, children)])
                }
                Rule::Container(name, _, query, children) if calls(&query.source()) => {
                    let name = name.as_deref().unwrap_or_default();
                    let query = self.functions(&query.source());
                    let parsed = Stylesheet::parse(&format!("@container {name} {query}{{}}"));
                    let children = self
                        .apply(children, parent, used)
                        .unwrap_or_else(|| children.clone());
                    Some(match parsed.0.first().map(|rule| &**rule) {
                        Some(Rule::Container(name, block, query, _)) => vec![Rule::Container(
                            name.clone(),
                            *block,
                            query.clone(),
                            children,
                        )],
                        _ => Vec::new(),
                    })
                }
                Rule::Starting(children) => self
                    .apply(children, parent, used)
                    .map(|children| vec![Rule::Starting(children)]),
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

fn imports(rule: &Rule) -> bool {
    match rule {
        Rule::Layer(_, children) => matches!(children.first(), Some(Rule::Tailwind(_))),
        _ => matches!(rule, Rule::Tailwind(_)),
    }
}

fn configures(rule: &Rule) -> bool {
    match rule {
        Rule::Tailwind(_)
        | Rule::Theme(..)
        | Rule::CustomVariant(..)
        | Rule::VariantBlock(..)
        | Rule::Utility(..) => true,
        Rule::Layer(_, children) => children.is_empty() || children.iter().any(configures),
        _ => false,
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

fn chunk(utilities: &[(Sort, u64, Vec<Rule>)], layer: &str) -> Rc<Stylesheet> {
    let rules = utilities
        .iter()
        .flat_map(|(.., rules)| rules.iter().cloned());
    Rc::new(Stylesheet(match layer {
        "" => rules.map(Rc::new).collect(),
        _ => vec![Rc::new(Rule::Layer(vec![layer.into()], rules.collect()))],
    }))
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
    let plain = |rule: &&Rc<Rule>| matches!(***rule, Rule::Layer(..)) || !configures(rule);
    let rules = sheets.iter().flat_map(|sheet| &sheet.0).filter(plain);
    rules.map(|rule| rule.source()).collect()
}
