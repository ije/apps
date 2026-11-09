use super::Document;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash, Hasher},
    rc::Rc,
};

pub(crate) type SelectorList = Rc<Vec<Selector>>;

pub(crate) const FOCUS: u16 = 1;
pub(crate) const FOCUS_WITHIN: u16 = 2;
pub(crate) const HOVER: u16 = 4;
pub(crate) const ACTIVE: u16 = 8;
pub(crate) const FOCUS_VISIBLE: u16 = 128;
const HAS: u16 = 16;

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
    NoNamespace,
}

// WARNING: page queries reject pseudo-classes and pseudo-elements outside these lists, as browsers
// do; stylesheets keep unknown ones as selectors that never match.
const PSEUDOS: [&str; 4] = [
    "active any-link autofill blank buffering checked closed default defined disabled empty enabled first-child first-of-type focus focus-visible focus-within fullscreen future has-slotted host hover in-range indeterminate invalid last-child last-of-type link local-link modal muted only-child only-of-type open optional out-of-range past paused picture-in-picture placeholder-shown playing popover-open read-only read-write required root scope seeking stalled target target-within user-invalid user-valid valid visited volume-locked -webkit-any-link -webkit-autofill -webkit-full-screen",
    "current dir has host host-context is lang not nth-child nth-last-child nth-last-of-type nth-of-type state where -webkit-any",
    "after backdrop before cue details-content file-selector-button first-letter first-line grammar-error marker placeholder selection spelling-error target-text view-transition",
    "cue highlight part slotted view-transition-group view-transition-image-pair view-transition-new view-transition-old",
];

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

type Matches = rustc_hash::FxHashMap<(usize, usize, Option<usize>, bool), (bool, Rc<[Dependency]>)>;

type Positions = Rc<HashMap<usize, [usize; 4]>>;

type NthPositions = Rc<(rustc_hash::FxHashMap<usize, usize>, Vec<Dependency>)>;

pub(crate) type Dependency = (u32, u32);

// WARNING: the `style` attribute has a kind of its own, so script that sets styles keeps the matches of the
// subtree. Only the node's own style and `[style]` selectors depend on it.
// WARNING: revisions come from one counter, so a freed node's snapshot can go: values that depended on it
// never match a node that later takes its id.
#[derive(Clone, Default)]
struct Snapshot {
    values: [u32; KINDS],
    revisions: [u32; KINDS],
}
const KINDS: usize = 7;

// WARNING: the attributes that selectors name, and of those the ones whose value they compare: a row's `data-line`
// number changes nothing when selectors only test that it is present.
#[derive(Default)]
pub(crate) struct Named {
    names: rustc_hash::FxHashSet<String>,
    valued: rustc_hash::FxHashSet<String>,
}

#[derive(Default)]
pub(crate) struct SelectorCache {
    pub(crate) attributes: Named,
    nodes: Vec<Snapshot>,
    seen: Vec<u64>,
    values: Matches,
    size: usize,
    positions: rustc_hash::FxHashMap<usize, (u32, Positions)>,
    nth: rustc_hash::FxHashMap<(usize, usize, Option<usize>), NthPositions>,
    pub(crate) dependencies: Vec<Dependency>,
    revision: u32,
    pub(crate) epoch: u64,
    changes: std::collections::VecDeque<(u64, Vec<u32>)>,
    // WARNING: `dirty` marks the nodes whose matches may have changed since a style pass last read them, and
    // `dependents` lists, per node, the nodes whose matches read it (with stale entries, as a superset).
    // An incremental pass styles only dirty nodes, so every recorded dependency must reach `dependents`.
    pub(crate) dirty: Vec<bool>,
    // WARNING: `children` holds the nodes that read the children list, so a change of only the children walks just
    // those: a list that rebuilds its rows changes its children every frame, and every row reads the list.
    dependents: Vec<(u32, Vec<u32>)>,
    children: Vec<(u32, Vec<u32>)>,
    // WARNING: by shadow root, its host. In its own tree a root stands for its host, which only `:host` matches.
    hosts: Rc<rustc_hash::FxHashMap<usize, usize>>,
}

impl SelectorCache {
    // WARNING: roots that join or leave the map match again, as their snapshots go.
    pub(crate) fn set_hosts(&mut self, hosts: &Rc<rustc_hash::FxHashMap<usize, usize>>) {
        if Rc::ptr_eq(&self.hosts, hosts) {
            return;
        }
        let moved: Vec<usize> = self
            .hosts
            .iter()
            .chain(hosts.iter())
            .filter(|(root, _)| self.hosts.get(root) != hosts.get(root))
            .map(|(&root, _)| root)
            .collect();
        for root in moved {
            self.forget(root);
        }
        self.hosts = hosts.clone();
    }

    // WARNING: results are keyed by selector address, so they must go with the scopes that held the selectors.
    // Positions of `:nth-child(An+B of S)` go as a children change of their parent, as in `update`.
    pub(crate) fn forget_selectors(&mut self) {
        self.values.clear();
        self.size = 0;
        let mut changes = Vec::new();
        for ((_, parent, _), _) in std::mem::take(&mut self.nth) {
            if let Some(slot) = self.nodes.get_mut(parent) {
                self.revision = self.revision.wrapping_add(1);
                slot.revisions[2] = self.revision;
                changes.push((parent * KINDS + 2) as u32);
            }
        }
        self.record(changes);
    }

    fn record(&mut self, changes: Vec<u32>) {
        for &change in &changes {
            self.mark(change as usize / KINDS, change as usize % KINDS == 2);
        }
        if !changes.is_empty() {
            self.epoch += 1;
            self.changes.push_back((self.epoch, changes));
            if self.changes.len() > 32 {
                self.changes.pop_front();
            }
        }
    }

    pub fn forget(&mut self, id: usize) {
        if let Some(snapshot) = self.nodes.get_mut(id) {
            *snapshot = Snapshot::default();
            self.seen[id] = 0;
            self.mark(id, false);
            // WARNING: what read the node is marked, so its lists go, or a node that takes its id marks them again.
            self.dependents[id] = Default::default();
            self.children[id] = Default::default();
        }
    }

    // WARNING: `id` is always marked, but with `children` only the nodes whose matches read its children are.
    pub(crate) fn mark(&mut self, id: usize, children: bool) {
        if let Some(dirty) = self.dirty.get_mut(id) {
            *dirty = true;
        }
        let lists = [&self.children, &self.dependents];
        for (_, list) in lists[..if children { 1 } else { 2 }]
            .iter()
            .filter_map(|lists| lists.get(id))
        {
            for &node in list {
                if let Some(dirty) = self.dirty.get_mut(node as usize) {
                    *dirty = true;
                }
            }
        }
    }

    /// Records that `id` read the nodes in `dependencies`, which must be sorted.
    pub(crate) fn register(&mut self, id: usize) {
        for group in self
            .dependencies
            .chunk_by(|a, b| a.0 as usize / KINDS == b.0 as usize / KINDS)
        {
            let node = group[0].0 as usize / KINDS;
            let lists = if group.iter().any(|&(kind, _)| kind as usize % KINDS == 2) {
                &mut self.children
            } else {
                &mut self.dependents
            };
            let Some((compacted, list)) = lists.get_mut(node).filter(|_| node != id) else {
                continue;
            };
            let entry = id as u32;
            if list.last() != Some(&entry) {
                list.push(entry);
                if list.len() >= 16.max(*compacted as usize * 2) {
                    // WARNING: forgotten nodes go too, so a long-lived parent doesn't list every row it ever had.
                    let seen = &self.seen;
                    list.retain(|&node| seen.get(node as usize).is_some_and(|&seen| seen != 0));
                    list.sort_unstable();
                    list.dedup();
                    *compacted = list.len() as u32;
                }
            }
        }
    }

    pub fn trim(&mut self, len: usize) {
        self.values = Default::default();
        self.size = 0;
        self.positions = Default::default();
        self.dependencies = Vec::new();
        self.nodes.truncate(len);
        self.nodes.shrink_to_fit();
        self.seen.truncate(len);
        self.seen.shrink_to_fit();
        self.dirty.truncate(len);
        self.dirty.fill(true);
        self.dependents.truncate(len);
        self.dependents.shrink_to_fit();
        self.children.truncate(len);
        self.children.shrink_to_fit();
    }

    pub fn update(&mut self, doc: &Document, order: &[(u32, u32)]) {
        if self.values.len() > 131072 {
            self.values.clear();
            self.size = 0;
        }
        if self.positions.len() > 65536 {
            self.positions.clear();
        }
        if self.nodes.len() < doc.nodes.len() {
            self.nodes.resize(doc.nodes.len(), Snapshot::default());
            self.seen.resize(doc.nodes.len(), 0);
        }
        self.dirty
            .resize(doc.nodes.len().max(self.dirty.len()), true);
        self.dependents
            .resize_with(doc.nodes.len().max(self.dependents.len()), Default::default);
        self.children
            .resize_with(doc.nodes.len().max(self.children.len()), Default::default);
        let mut changes = Vec::new();
        let mut retagged = Vec::new();
        for &(id, _) in order {
            let id = id as usize;
            let revision = doc.nodes.revision(id);
            let fresh = std::mem::replace(&mut self.seen[id], revision) != revision;
            if !fresh && !cfg!(debug_assertions) {
                continue;
            }
            let node = &doc.nodes[id];
            let Snapshot { values, revisions } = &mut self.nodes[id];
            let hash = rustc_hash::FxBuildHasher;
            let tag = hash.hash_one(&node.tag);
            let attributes = node
                .attrs
                .iter()
                // WARNING: only selectors read these, as pseudo-classes read none of them, so a list that recycles
                // its rows keeps the matches of rows that changed only their labels.
                .filter_map(|(key, value)| {
                    let selected = key.starts_with("data-")
                        || key.starts_with("aria-")
                        || matches!(key.as_str(), "key" | "title" | "role" | "tabindex");
                    (key.as_str() != "style" && (!selected || self.attributes.names.contains(key.as_str())))
                        .then(|| (key, (!selected || self.attributes.valued.contains(key.as_str())).then_some(value)))
                })
                .fold(rustc_hash::FxHasher::default(), |mut state, pair| {
                    pair.hash(&mut state);
                    state
                });
            let new = [
                hash.hash_one((tag, attributes.finish())) as u32,
                node.parent.map_or(0, |parent| parent as u32 + 1),
                hash.hash_one(&node.children) as u32,
                u32::from(!node.text.is_empty())
                    | node
                        .value
                        .as_deref()
                        .map_or(0, |value| 2 << u32::from(value.is_empty())),
                tag as u32,
                u32::from(node.state),
                hash.hash_one((node.attrs.get("style"), &node.inline)) as u32,
            ];
            // WARNING: the attribute hash reads only attributes that selectors name, so a new stylesheet changes it.
            debug_assert!(
                fresh || new[1..] == values[1..],
                "node {id} changed without a new revision"
            );
            for (kind, (revision, (old, new))) in revisions
                .iter_mut()
                .zip(values.iter_mut().zip(new))
                .enumerate()
            {
                if *old != new {
                    if kind == 4
                        && let Some(parent) = node.parent
                    {
                        retagged.push(parent);
                    }
                    *old = new;
                    self.revision = self.revision.wrapping_add(1);
                    *revision = self.revision;
                    changes.push((id * KINDS + kind) as u32);
                }
            }
        }
        // WARNING: sibling selectors depend only on the parent's children, so a tag change counts as one.
        for parent in retagged {
            if let Some(slot) = self.nodes.get_mut(parent) {
                self.revision = self.revision.wrapping_add(1);
                slot.revisions[2] = self.revision;
                changes.push((parent * KINDS + 2) as u32);
            }
        }
        // WARNING: `:nth-child(An+B of S)` results depend only on their parent's children, so a change to anything
        // that S read on the siblings drops the cached positions and counts as a children change. Never drop
        // positions without that change.
        let mut checked = 0;
        while checked < changes.len() && !self.nth.is_empty() {
            let mut fresh = changes[checked..].to_vec();
            fresh.sort_unstable();
            checked = changes.len();
            let all = self.nth.len() > 65536;
            let mut stale = Vec::new();
            self.nth.retain(|&(_, parent, _), positions| {
                let dependencies = &positions.1;
                let keep = !all
                    && !if fresh.len() < dependencies.len() {
                        fresh
                            .iter()
                            .any(|id| dependencies.binary_search_by_key(id, |&(id, _)| id).is_ok())
                    } else {
                        dependencies
                            .iter()
                            .any(|(id, _)| fresh.binary_search(id).is_ok())
                    };
                if !keep {
                    stale.push(parent);
                }
                keep
            });
            for parent in stale {
                if let Some(slot) = self.nodes.get_mut(parent) {
                    self.revision = self.revision.wrapping_add(1);
                    slot.revisions[2] = self.revision;
                    changes.push((parent * KINDS + 2) as u32);
                }
            }
        }
        self.record(changes);
    }

    pub(crate) fn depend(&mut self, id: usize, kind: usize) {
        if let Some(node) = self.nodes.get(id) {
            self.dependencies
                .push(((id * KINDS + kind) as u32, node.revisions[kind]));
        }
    }

    pub(crate) fn signature(&self, id: usize) -> [u32; KINDS] {
        self.nodes.get(id).map_or([0; KINDS], |node| node.values)
    }

    pub(crate) fn reads(&self, id: usize, kind: usize) -> bool {
        let key = (id * KINDS + kind) as u32;
        self.dependencies.iter().any(|&(read, _)| read == key)
    }

    // WARNING: the matches just made for `id` hold for a sibling with the same values only when they read nothing but
    // `id` and its flat tree ancestors, and no children list of `id` or its parent, which positions and siblings read.
    pub(crate) fn shareable(&self, doc: &Document, id: usize) -> bool {
        let parent = doc.nodes[id].parent;
        let mut chain = [0usize; 64];
        let mut length = 0;
        let mut up = parent;
        while let Some(node) = up {
            if length == chain.len() {
                return false;
            }
            chain[length] = node;
            length += 1;
            up = doc.nodes[node].parent.or_else(|| self.hosts.get(&node).copied());
        }
        self.dependencies.iter().all(|&(read, _)| {
            let (node, kind) = (read as usize / KINDS, read as usize % KINDS);
            if node == id {
                kind != 2
            } else {
                !(Some(node) == parent && kind == 2) && chain[..length].contains(&node)
            }
        })
    }

    // The dependencies of matches that `from` shared, as reads of `to` at its revisions.
    pub(crate) fn transfer(&mut self, from: usize, to: usize, dependencies: &[Dependency]) {
        self.dependencies.clear();
        for &(read, revision) in dependencies {
            let (node, kind) = (read as usize / KINDS, read as usize % KINDS);
            self.dependencies.push(if node == from {
                ((to * KINDS + kind) as u32, self.nodes[to].revisions[kind])
            } else {
                (read, revision)
            });
        }
        self.dependencies.sort_unstable();
    }

    pub(crate) fn valid(&self, dependencies: &[Dependency]) -> bool {
        dependencies.iter().all(|&(id, revision)| {
            let id = id as usize;
            self.nodes
                .get(id / KINDS)
                .map_or(0, |node| node.revisions[id % KINDS])
                == revision
        })
    }

    fn reserve(&mut self, count: usize) {
        self.size += count;
        if self.size > 2_000_000 {
            self.values.clear();
            self.size = count;
        }
    }

    pub(crate) fn valid_since(
        &self,
        dependencies: &[Dependency],
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
    fn argument(
        name: &str,
        argument: &str,
        parent: Option<&SelectorList>,
        strict: bool,
    ) -> Option<(Vec<Self>, [i32; 2])> {
        let list = |source: &str| {
            Self::list(source, parent, strict, false).filter(|list| !list.is_empty())
        };
        match name {
            "is" | "where" => Some((
                split(argument)
                    .into_iter()
                    .flat_map(|part| list(part).unwrap_or_default())
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
                let selectors = list(&relative.join(","))?;
                // WARNING: nested `:has()` is invalid and makes matching cost O(N²) or more.
                if selectors
                    .iter()
                    .any(|selector| selector.states(&mut Vec::new()) & HAS != 0)
                {
                    return None;
                }
                Some((selectors, [0, 0]))
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
                    if let Some((selectors, _)) = Self::argument(name, argument, parent, false) {
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

    pub(crate) fn attributes(&self, names: &mut Named) {
        if let Some((_, previous)) = &self.previous {
            previous.attributes(names);
        }
        for simple in &self.simple {
            match simple {
                SimpleSelector::Attribute(name, operator, ..) => {
                    let name = name.to_ascii_lowercase();
                    if !operator.is_empty() {
                        names.valued.insert(name.clone());
                    }
                    names.names.insert(name);
                }
                SimpleSelector::Pseudo(_, _, parsed, _) => {
                    for selector in parsed {
                        selector.attributes(names);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn states(&self, parents: &mut Vec<SelectorList>) -> u16 {
        let mut states = self
            .previous
            .as_ref()
            .map_or(0, |(_, previous)| previous.states(parents));
        for simple in &self.simple {
            match simple {
                SimpleSelector::Pseudo(name, _, parsed, _) => {
                    states |= match name.as_str() {
                        "hover" => HOVER,
                        "active" => ACTIVE,
                        "focus" | "focus-visible" => FOCUS,
                        "focus-within" => FOCUS_WITHIN,
                        "has" => HAS,
                        _ => 0,
                    };
                    for selector in parsed {
                        states |= selector.states(parents);
                    }
                }
                SimpleSelector::Parent(list, _) => parents.push(list.clone()),
                _ => {}
            }
        }
        states
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
                    "placeholder",
                    "-webkit-scrollbar",
                    "-webkit-scrollbar-track",
                    "-webkit-scrollbar-thumb",
                    "-webkit-scrollbar-corner",
                    "-webkit-scrollbar-track-piece",
                    "slotted",
                    "part",
                    "starting-style",
                    "backdrop",
                ]
                .iter()
                .position(|n| n == name)?,
            ))
        })?;
        let mut selector = self.clone();
        // WARNING: a `::backdrop` rule in `@starting-style` is kind 16.
        if kind == 15
            && matches!(&self.simple[index + 1..], [SimpleSelector::Element(name, _)]
                if name == "starting-style")
        {
            selector.simple.truncate(index);
            return Some((16, selector));
        }
        // WARNING: `shadow_target()` reads what follows `::slotted()` and `::part()`.
        if matches!(kind, 12 | 13) {
            selector.simple.truncate(index);
            return Some((kind, selector));
        }
        if !(7..12).contains(&kind) {
            selector.simple.remove(index);
            return Some((kind, selector));
        }
        // WARNING: scrollbar parts keep only `:hover` on the thumb (kind 11); rules with other scrollbar states never match.
        let kind = match (kind, &self.simple[index + 1..]) {
            (11, []) => 8,
            (_, []) => kind,
            (9, [SimpleSelector::Pseudo(name, ..)]) if name == "hover" => 11,
            _ => return None,
        };
        selector.simple.truncate(index);
        Some((kind, selector))
    }

    // WARNING: for `::slotted()` and `::part()`, the pseudo-element they style (`usize::MAX` for the element, 14 and 16
    // in `@starting-style`) and the pseudo-classes that a part must match. As in Chrome, `::slotted()` takes `::before`,
    // `::after`, `::marker`, `::placeholder` and `::backdrop`, and `::part()` also the other pseudo-elements and
    // non-structural pseudo-classes.
    pub(crate) fn shadow_target(&self) -> Option<(usize, Self)> {
        let index = self
            .simple
            .iter()
            .position(|part| matches!(part, SimpleSelector::Element(..)))?;
        let part =
            matches!(&self.simple[index], SimpleSelector::Element(name, _) if name == "part");
        let rest = &self.simple[index + 1..];
        let split = rest
            .iter()
            .position(|part| matches!(part, SimpleSelector::Element(..)))
            .unwrap_or(rest.len());
        let compound = |simple: &[SimpleSelector]| Self {
            simple: simple.to_vec(),
            previous: None,
            specificity: [0; 3],
        };
        let kind = match compound(&rest[split..]).without_pseudo_element() {
            _ if split == rest.len() => usize::MAX,
            Some((kind, left)) if left.simple.is_empty() => kind,
            _ => return None,
        };
        let classes = compound(&rest[..split]);
        let allowed = if part {
            !matches!(kind, 12 | 13) && classes.stateful()
        } else {
            matches!(kind, 0 | 1 | 3 | 6 | 14..=16 | usize::MAX) && classes.simple.is_empty()
        };
        allowed.then_some((kind, classes))
    }

    fn stateful(&self) -> bool {
        self.previous.is_none()
            && self.simple.iter().all(|simple| {
                matches!(simple, SimpleSelector::Pseudo(name, _, parsed, _)
                    if !(name.ends_with("child")
                        || name.ends_with("of-type")
                        || matches!(name.as_str(), "empty" | "has" | "host" | "host-context" | "scope"))
                        && parsed.iter().all(Self::stateful))
            })
    }

    pub(crate) fn pseudo_argument(&self) -> Option<&str> {
        self.simple.iter().find_map(|part| match part {
            SimpleSelector::Element(_, argument) => Some(argument.as_str()),
            _ => None,
        })
    }

    pub(crate) fn key(&self) -> Option<(u8, &str)> {
        self.simple
            .iter()
            .filter_map(|selector| match selector {
                SimpleSelector::Id(id) => Some((0, id.as_str())),
                SimpleSelector::Class(class) => Some((1, class.as_str())),
                // WARNING: keys match node tags and attribute names exactly, so mixed-case names, which also match lowercase HTML ones, have none.
                SimpleSelector::Tag(name) if !name.bytes().any(|c| c.is_ascii_uppercase()) => {
                    Some((2, name.as_str()))
                }
                SimpleSelector::Attribute(name, ..)
                    if !name.bytes().any(|c| c.is_ascii_uppercase()) =>
                {
                    Some((3, name.as_str()))
                }
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
                SimpleSelector::NoNamespace => source.push('|'),
            }
        }
        source
    }

    pub(crate) fn parse(source: &str, parent: Option<&SelectorList>) -> Vec<Self> {
        Self::list(source, parent, false, true).unwrap_or_default()
    }

    pub(crate) fn try_parse(source: &str) -> anyhow::Result<Vec<Self>> {
        Self::list(source, None, true, true)
            .ok_or_else(|| anyhow::anyhow!("SyntaxError: '{source}' is not a valid selector"))
    }

    // WARNING: lists past the nesting and length limits give no selectors instead of `None`.
    fn list(
        source: &str,
        parent: Option<&SelectorList>,
        strict: bool,
        nested: bool,
    ) -> Option<Vec<Self>> {
        let mut depth = 0u32;
        if source.bytes().any(|c| {
            depth = match c {
                b'(' => depth + 1,
                b')' => depth.saturating_sub(1),
                _ => depth,
            };
            depth > 32
        }) {
            return Some(Vec::new());
        }
        let mut selectors = Vec::new();
        let parent_specificity = parent
            .and_then(|p| p.iter().map(|s| s.specificity).max())
            .unwrap_or_default();
        for source in split(source) {
            let mut rest = source.trim();
            if rest.is_empty() {
                return None;
            }
            let mut previous = parent
                .filter(|_| nested && (!rest.contains('&') || rest.starts_with(['>', '+', '~'])))
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
                    return Some(Vec::new());
                }
                let mut combinator = ' ';
                if rest.starts_with(['>', '+', '~']) {
                    previous.as_ref()?;
                    combinator = rest.as_bytes()[0] as char;
                    rest = rest[1..].trim_start();
                }
                let end = selector_boundary(rest, |c| {
                    c.is_ascii_whitespace() || matches!(c, '>' | '+' | '~')
                });
                if end == 0 {
                    return None;
                }
                let mut compound = &rest[..end];
                let none = compound.starts_with('|');
                if let Some(tail) = compound
                    .strip_prefix("*|")
                    .or_else(|| compound.strip_prefix('|'))
                {
                    if !tail.starts_with('*') && identifier(tail).is_none() {
                        return None;
                    }
                    compound = tail;
                } else if compound.contains('|')
                    && let Some((_, length)) = identifier(compound)
                    && compound[length..].starts_with('|')
                {
                    return None;
                }
                let mut simple = Vec::new();
                let mut parent_index = None;
                let mut specificity = previous.as_ref().map_or([0; 3], |s| s.specificity);
                while !compound.is_empty() {
                    let marker = compound.as_bytes()[0];
                    if marker == b'[' {
                        let end = 1 + selector_boundary(&compound[1..], |c| c == ']');
                        if end == compound.len() {
                            return None;
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
                        let (name, length) = identifier(attribute)?;
                        let tail = attribute[length..].trim_start();
                        let (operator, value, insensitive) = if tail.is_empty() {
                            ("", String::new(), false)
                        } else {
                            let operator = ["=", "~=", "|=", "^=", "$=", "*="]
                                .into_iter()
                                .find(|operator| tail.starts_with(operator))?;
                            let tail = tail[operator.len()..].trim_start();
                            let (value, length) = if let Some(quote) =
                                tail.chars().next().filter(|c| matches!(c, '"' | '\''))
                            {
                                let (value, length) = unescape(&tail[1..], |c| c == quote);
                                if length + 1 == tail.len() {
                                    return None;
                                }
                                (value, length + 2)
                            } else {
                                identifier(tail)?
                            };
                            let insensitive = match tail[length..].trim_start() {
                                "" | "s" | "S" => false,
                                "i" | "I" => true,
                                _ => return None,
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
                            return None;
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
                        let (name, length) = identifier(compound)?;
                        let name = name.to_ascii_lowercase();
                        compound = &compound[length..];
                        let mut argument = "";
                        let functional = compound.starts_with('(');
                        if functional {
                            let close = 1 + selector_boundary(&compound[1..], |c| c == ')');
                            if close == compound.len() {
                                return None;
                            }
                            argument = &compound[1..close];
                            compound = &compound[close + 1..];
                        }
                        let element = element
                            || matches!(
                                name.as_str(),
                                "before" | "after" | "first-line" | "first-letter"
                            );
                        if strict
                            && !PSEUDOS[usize::from(element) * 2 + usize::from(functional)]
                                .split(' ')
                                .any(|known| known == name)
                            && !(element && !functional && name.starts_with("-webkit-"))
                        {
                            return None;
                        }
                        if element {
                            specificity[2] = specificity[2].saturating_add(1);
                            simple.push(SimpleSelector::Element(name, argument.into()));
                            continue;
                        }
                        if name == "root" {
                            specificity[1] = specificity[1].saturating_add(1);
                            simple.push(SimpleSelector::Root);
                            continue;
                        }
                        let (selectors, nth) = Self::argument(&name, argument, parent, strict)?;
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
                        return None;
                    }
                    let (name, length) = identifier(compound)?;
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
                if none {
                    simple.insert(0, SimpleSelector::NoNamespace);
                }
                previous = Some(Self {
                    simple,
                    previous: previous.map(|s| (combinator, Box::new(s))),
                    specificity,
                });
                rest = rest[end..].trim_start();
            }
            selectors.push(previous?);
        }
        Some(selectors)
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

    // WARNING: for elements with no matched rules yet, so no memo entry of their own: the entry would only serve a
    // later pass in which none of the node's own values changed, and pages that build rows each frame never have one.
    // Steps up the tree still use and fill the memo, which siblings share.
    pub(crate) fn matches_fresh(&self, doc: &Document, id: usize, memo: &mut SelectorCache) -> bool {
        self.matches_with(doc, id, None, memo, false)
    }

    // WARNING: like `matches_cached` on an element: only `*` and no combinator matches every element, so callers
    // skip matching. A shadow root is featureless, so it must not take this.
    pub(crate) fn always(&self) -> bool {
        self.previous.is_none()
            && self
                .simple
                .iter()
                .all(|simple| matches!(simple, SimpleSelector::Universal))
    }

    // WARNING: `:host` matches only the shadow root that stands for its host, never an element.
    pub(crate) fn host_only(&self) -> bool {
        self.previous.is_none()
            && self
                .simple
                .iter()
                .any(|simple| matches!(simple, SimpleSelector::Pseudo(name, ..) if name == "host"))
    }

    pub(crate) fn matches_cached(
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
    ) -> bool {
        self.matches_with(doc, id, scope, memo, true)
    }

    fn matches_with(
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
        store: bool,
    ) -> bool {
        let key = (self as *const Self as usize, id, scope, false);
        let compound = !store
            || self.previous.is_none()
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
        if self.simple.iter().any(|simple| {
            matches!(simple, SimpleSelector::Attribute(name, ..) if name.eq_ignore_ascii_case("style"))
        }) {
            memo.depend(id, 6);
        }
        let result = (|| {
            let node = &doc.nodes[id];
            let host = node
                .tag
                .starts_with('#')
                .then(|| memo.hosts.get(&id).copied())
                .flatten();
            if let Some(host) = host {
                // WARNING: the host is featureless in its shadow tree: only `:host`, and lists that hold it,
                // match the root.
                let mut hosted = false;
                for simple in &self.simple {
                    let matched = match simple {
                        SimpleSelector::Universal => continue,
                        SimpleSelector::Pseudo(name, _, parsed, _) if name == "host" => {
                            parsed.is_empty()
                                || parsed
                                    .iter()
                                    .any(|s| s.matches_cached(doc, host, scope, memo))
                        }
                        SimpleSelector::Pseudo(name, _, parsed, _)
                            if matches!(name.as_str(), "is" | "where") =>
                        {
                            parsed
                                .iter()
                                .any(|s| s.matches_cached(doc, id, scope, memo))
                        }
                        SimpleSelector::Parent(parent, _) => parent
                            .iter()
                            .any(|s| s.matches_cached(doc, id, scope, memo)),
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
                || !self.simple.iter().all(|s| match s {
                    // WARNING: HTML tags and attribute names are lowercase in the tree and match selectors ASCII case-insensitively; SVG ones keep their case and match as written.
                    SimpleSelector::Tag(tag) => {
                        node.tag == *tag
                            || node.tag.eq_ignore_ascii_case(tag)
                                && !node.tag.bytes().any(|c| c.is_ascii_uppercase())
                    }
                    SimpleSelector::Id(id) => node.attrs.get("id") == Some(id),
                    SimpleSelector::Class(class) => {
                        node.attrs.get("class").is_some_and(|classes| {
                            classes.split_ascii_whitespace().any(|c| c == class)
                        })
                    }
                    SimpleSelector::Universal => true,
                    SimpleSelector::Root => node.tag == "html",
                    SimpleSelector::Attribute(name, operator, value, insensitive) => node
                        .attrs
                        .iter()
                        .find(|(key, _)| {
                            *key == name
                                || key.eq_ignore_ascii_case(name)
                                    && !key.bytes().any(|c| c.is_ascii_uppercase())
                        })
                        .is_some_and(|(_, actual)| {
                            let equal = |part: Option<&str>| {
                                part.is_some_and(|part| {
                                    if *insensitive {
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
                                "*=" if *insensitive => actual
                                    .as_bytes()
                                    .windows(length)
                                    .any(|part| part.eq_ignore_ascii_case(value.as_bytes())),
                                "*=" => actual.contains(value.as_str()),
                                _ => false,
                            }
                        }),
                    SimpleSelector::Pseudo(name, argument, parsed, [a, b]) => match name.as_str() {
                        // WARNING: a document scope matches its root element, as in browsers.
                        "scope" => scope.map_or(node.tag == "html", |scope| {
                            scope == id
                                || node.parent == Some(scope) && doc.nodes[scope].tag == "#document"
                        }),
                        "has" => {
                            let (mut children, mut siblings, mut depth) = (false, 0usize, 0usize);
                            for selector in parsed {
                                let (mut levels, mut steps, mut current) =
                                    (0usize, 0usize, selector);
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
                            if siblings > 0
                                && let Some(parent) = node.parent
                            {
                                memo.depend(id, 1);
                                memo.depend(parent, 2);
                                pending.extend(
                                    doc.nodes[parent]
                                        .children
                                        .iter()
                                        .skip_while(|&&sibling| sibling != id)
                                        .skip(1)
                                        .filter(|&&sibling| {
                                            !doc.nodes[sibling].tag.starts_with('#')
                                        })
                                        .take(siblings)
                                        .map(|&sibling| (sibling, 0)),
                                );
                            }
                            let mut found = false;
                            while let Some((child, level)) = pending.pop() {
                                if parsed.iter().any(|selector| {
                                    selector.matches_cached(doc, child, Some(id), memo)
                                }) {
                                    found = true;
                                    break;
                                }
                                if level < depth {
                                    memo.depend(child, 2);
                                    pending.extend(
                                        doc.nodes[child]
                                            .children
                                            .iter()
                                            .map(|&grandchild| (grandchild, level + 1)),
                                    );
                                }
                            }
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
                                    "focus-visible" => FOCUS_VISIBLE,
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
                        "checked" => {
                            memo.depend(id, 0);
                            memo.depend(id, 5);
                            match node.tag.as_str() {
                                "input" => {
                                    node.checked()
                                        && ["checkbox", "radio"].iter().any(|kind| {
                                            node.input_type().eq_ignore_ascii_case(kind)
                                        })
                                }
                                "option" if node.state & super::DIRTY != 0 || node.checked() => {
                                    node.checked()
                                }
                                // WARNING: until a script syncs the selection, a single select without a selected option shows its first enabled one.
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
                                        && options
                                            .iter()
                                            .all(|&option| !doc.nodes[option].checked())
                                        && options.iter().find(|&&option| {
                                            !doc.nodes[option].attrs.contains_key("disabled")
                                        }) == Some(&id)
                                }
                                _ => false,
                            }
                        }
                        "modal" | "popover-open" | "fullscreen" | "-webkit-full-screen" => {
                            memo.depend(id, 5);
                            node.state
                                & match name.as_str() {
                                    "modal" => super::MODAL,
                                    "popover-open" => super::POPOVER,
                                    _ => super::FULLSCREEN,
                                }
                                != 0
                        }
                        // WARNING: for the UA sheet: a modal dialog keeps its place until it leaves the top layer.
                        "-internal-dialog-in-top-layer" => {
                            memo.depend(id, 5);
                            node.tag == "dialog"
                                && node.state & (super::MODAL | super::MODAL << 5) != 0
                        }
                        "-internal-sf-symbol" => node.sf_symbol().is_some(),
                        "-internal-native-control" => node.native_control().is_some(),
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
                                            doc.nodes[child].tag == "#text"
                                                && doc.nodes[child].text.is_empty()
                                        }),
                                    str::is_empty,
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
                                    || ![
                                        "hidden", "checkbox", "radio", "file", "submit", "image",
                                        "reset", "button", "color", "range",
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
                                let siblings = &doc.nodes[parent].children;
                                let (index, count) = if parsed.is_empty() {
                                    let revision =
                                        memo.nodes.get(parent).map_or(0, |node| node.revisions[2]);
                                    let positions = match memo.positions.get(&parent) {
                                        Some((cached, positions)) if *cached == revision => {
                                            positions.clone()
                                        }
                                        _ => {
                                            let mut types = HashMap::<&str, usize>::new();
                                            let mut positions = HashMap::new();
                                            for &child in siblings {
                                                let tag = doc.nodes[child].tag.as_str();
                                                if !tag.starts_with('#') {
                                                    let of_type = types.entry(tag).or_default();
                                                    positions.insert(
                                                        child,
                                                        [positions.len(), 0, *of_type, 0],
                                                    );
                                                    *of_type += 1;
                                                }
                                            }
                                            let count = positions.len();
                                            for (child, position) in &mut positions {
                                                position[1] = count;
                                                position[3] = types[doc.nodes[*child].tag.as_str()];
                                            }
                                            let positions = Rc::new(positions);
                                            memo.positions
                                                .insert(parent, (revision, positions.clone()));
                                            positions
                                        }
                                    };
                                    let Some(&[index, count, of_type, type_count]) =
                                        positions.get(&id)
                                    else {
                                        return false;
                                    };
                                    if name.ends_with("of-type") {
                                        (of_type, type_count)
                                    } else {
                                        (index, count)
                                    }
                                } else {
                                    if !parsed
                                        .iter()
                                        .any(|s| s.matches_cached(doc, id, scope, memo))
                                    {
                                        return false;
                                    }
                                    // WARNING: siblings match S once per parent, and the result depends on the
                                    // parent's children only; `update` turns changes of the positions into those.
                                    let key = (parsed.as_ptr() as usize, parent, scope);
                                    let positions = match memo.nth.get(&key) {
                                        Some(positions) => positions.clone(),
                                        None => {
                                            let start = memo.dependencies.len();
                                            memo.depend(parent, 2);
                                            let mut indices = rustc_hash::FxHashMap::default();
                                            for &child in siblings {
                                                if !doc.nodes[child].tag.starts_with('#')
                                                    && parsed.iter().any(|s| {
                                                        s.matches_cached(doc, child, scope, memo)
                                                    })
                                                {
                                                    indices.insert(child, indices.len());
                                                }
                                            }
                                            let mut dependencies =
                                                memo.dependencies.split_off(start);
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
                        // WARNING: an ancestor's entry holds what was read from it upward, not what the nodes below it
                        // read, or siblings that reach it through the memo take reads of each other's branches.
                        let from = memo.dependencies.len();
                        memo.depend(id, 1);
                        let key = (&**previous as *const Self as usize, id, scope, true);
                        if let Some((result, dependencies)) = memo.values.get(&key)
                            && memo.valid(dependencies)
                        {
                            matched = *result;
                            memo.dependencies.extend(dependencies.iter().copied());
                            break;
                        }
                        visited.push((key, from));
                        if previous.matches_cached(doc, id, scope, memo) {
                            matched = true;
                            break;
                        }
                        ancestor = doc.nodes[id].parent;
                    }
                    for (key, from) in visited.into_iter().rev() {
                        let mut dependencies = memo.dependencies[from..].to_vec();
                        dependencies.sort_unstable();
                        dependencies.dedup();
                        let dependencies: Rc<[_]> = dependencies.into();
                        memo.reserve(dependencies.len());
                        memo.values.insert(key, (matched, dependencies));
                    }
                    let mut dependencies = memo.dependencies[start..].to_vec();
                    dependencies.sort_unstable();
                    dependencies.dedup();
                    memo.dependencies.truncate(start);
                    memo.dependencies.extend(&dependencies);
                    matched
                }
            }
        })();
        if compound {
            return result;
        }
        let mut dependencies = memo.dependencies[start..].to_vec();
        dependencies.sort_unstable();
        dependencies.dedup();
        memo.dependencies.truncate(start);
        memo.dependencies.extend(&dependencies);
        memo.reserve(dependencies.len());
        memo.values.insert(key, (result, dependencies.into()));
        result
    }
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
        let doc = Document::parse(
            "<ul><li id=a><b></b></li><li id=b class=x><i><b></b></i></li><li id=c></li></ul>",
        );
        for (selector, expected) in [
            ("li:has(> b)", "a"),
            ("li:has(b)", "a,b"),
            ("li:has(> i b)", "b"),
            ("li:has(+ .x)", "a"),
            ("li:has(+ li > i)", "a"),
            ("li:has(+ li b)", "a"),
            ("li:has(+ li + li)", "a"),
            ("li:has(~ #c)", "a,b"),
            ("li:has(> b, + #c)", "a,b"),
        ] {
            assert_eq!(matched(&doc, selector), expected, "{selector}");
        }
        let doc = Document::parse("<html><body><p></p></body></html>");
        let body = doc.nodes.iter().position(|n| n.tag == "body").unwrap();
        let html = doc.nodes.iter().position(|n| n.tag == "html").unwrap();
        let selector = &Selector::parse(":scope > body", None)[0];
        assert!(selector.matches_scope(&doc, body, Some(0)));
        assert!(Selector::parse(":scope", None)[0].matches_scope(&doc, html, Some(0)));
    }

    #[test]
    fn nth_child_of_selector_follows_class_and_child_changes() {
        let mut doc = Document::parse(&format!(
            "<style>li:nth-child(odd of :not(.hidden)){{color:red}}li:nth-last-child(1 of :not(.hidden)){{width:1px}}li:nth-child(even of :has(> b)){{height:2px}}</style><ul><li><b></b></li><li><b></b></li>{}</ul>",
            "<li></li>".repeat(6)
        ));
        let [items, marks] = ["li", "b"].map(|tag| {
            (0..doc.nodes.len())
                .filter(|&id| doc.nodes[id].tag == tag)
                .collect::<Vec<_>>()
        });
        let (mut sheets, mut cache) = (HashMap::new(), crate::style::StyleCache::default());
        for (step, (hidden, owners)) in [
            (&[][..], [0, 1]),
            (&[0, 3], [0, 4]),
            (&[3], [2, 4]),
            (&[7], [2, 5]),
            (&[], [6, 7]),
        ]
        .into_iter()
        .enumerate()
        {
            for (index, &id) in items.iter().enumerate() {
                let value = if hidden.contains(&index) {
                    "hidden"
                } else {
                    ""
                };
                if doc.nodes[id].attrs.get("class").map_or("", String::as_str) != value {
                    doc.nodes[id].attrs.set("class", value);
                }
            }
            for (&mark, owner) in marks.iter().zip(owners) {
                let (old, new) = (doc.nodes[mark].parent.unwrap(), items[owner]);
                if old != new {
                    doc.nodes[old].children.retain(|&child| child != mark);
                    doc.nodes[new].children.push(mark);
                    doc.nodes[mark].parent = Some(new);
                }
            }
            let cached =
                crate::style::compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
            let fresh = crate::style::compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut crate::style::StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "step {step}");
            let shown: Vec<_> = (0..8).filter(|index| !hidden.contains(index)).collect();
            for (index, &id) in items.iter().enumerate() {
                let style = &cached.values[id];
                let position = shown.iter().position(|&shown| shown == index);
                assert_eq!(
                    style.get(crate::style::Prop::COLOR) == Some("red"),
                    position.is_some_and(|position| position % 2 == 0),
                    "step {step}: item {index}"
                );
                assert_eq!(
                    style.get(crate::style::Prop::WIDTH) == Some("1px"),
                    shown.last() == Some(&index),
                    "step {step}: last {index}"
                );
                assert_eq!(
                    style.get(crate::style::Prop::HEIGHT) == Some("2px"),
                    index == owners[1],
                    "step {step}: marked {index}"
                );
            }
        }
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
        let doc = Document::fragment(
            "<div id=x data-k='ABC def' lang='en-US' title='a]b c' class='hover:bg md:w-1/2 123'></div>",
            "body",
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
            ":has(:has(a))",
            ":has(:not(:has(a)))",
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
            ":is(:has(a)):not(:has(b))",
        ] {
            assert!(!Selector::parse(selector, None).is_empty(), "{selector}");
        }
        assert!(Selector::parse(&"a ".repeat(2000), None).is_empty());
    }

    #[test]
    fn page_queries_reject_invalid_selectors() {
        for selector in [
            "",
            " ",
            "div[",
            "li,",
            ",li",
            "li >",
            "> li",
            "+ li",
            ":foo",
            "::foo",
            "li:foo(1)",
            "li::before()",
            ":hover()",
            ":root()",
            ":is",
            ":has(:has(a))",
            ":has(:is(:has(a)))",
            ":not(:foo)",
            ":nth-child(2 of :foo)",
            "a)",
            "(a",
            ":not(a",
            "a[x=\"y]",
            "'a'",
            "a..b",
            "#1a",
            "svg|rect",
            "*|.a",
            "|",
            "|.a",
            "a|",
            "div|*",
            "*|",
            "[ns|a]",
            "a > > b",
            "li:nth-child",
            "a!",
        ] {
            let error = Selector::try_parse(selector).unwrap_err().to_string();
            assert!(error.starts_with("SyntaxError: "), "{selector}: {error}");
        }
        for selector in [
            "li",
            ":is()",
            ":is(:foo, li)",
            ":where(.a, 1x)",
            "#\\31 a",
            "*|rect",
            "*|*",
            "|rect",
            "|*",
            "[*|title]",
            "[|title]",
            "a::before",
            "a:before",
            "::-webkit-scrollbar-thumb:hover",
            "::-webkit-anything",
            "::slotted(span)",
            "::part(label)",
            ":host",
            ":host(.a)",
            ":host-context(.dark) b",
            ":focus-visible",
            ":visited",
            ":has(> img, + p)",
            "li:nth-child(2n+1 of .k)",
            ":-webkit-any(a, b)",
            "DIV > Span.A",
        ] {
            assert!(Selector::try_parse(selector).is_ok(), "{selector}");
        }
        let deep = format!("{}li{}", ":not(".repeat(1000), ")".repeat(1000));
        assert!(Selector::try_parse(&deep).unwrap().is_empty());
        assert!(Selector::try_parse(&format!(":not({})", "a ".repeat(300))).is_err());
        for selector in [":unknown", "::-moz-selection", "li:foo(1)"] {
            assert!(!Selector::parse(selector, None).is_empty(), "{selector}");
        }
    }

    #[test]
    fn html_names_match_case_insensitively_and_svg_names_as_written() {
        let doc = Document::parse(
            "<ul><li id=a data-v=1></li></ul><svg id=s viewBox='0 0 1 1'><linearGradient id=g></linearGradient><rect id=r></rect></svg>",
        );
        let query = |selector: &str| {
            let selectors = Selector::try_parse(selector).unwrap();
            (0..doc.nodes.len())
                .filter(|&id| selectors.iter().any(|s| s.matches(&doc, id)))
                .filter_map(|id| doc.nodes[id].attrs.get("id").cloned())
                .collect::<Vec<_>>()
                .join(",")
        };
        for (selector, expected) in [
            ("LI", "a"),
            ("Ul > lI", "a"),
            ("[DATA-V]", "a"),
            ("[Data-V='1']", "a"),
            ("[data-v='1' i]", "a"),
            ("linearGradient", "g"),
            ("lineargradient", ""),
            ("LINEARGRADIENT", ""),
            ("rect", "r"),
            ("[viewBox]", "s"),
            ("[viewbox]", ""),
            ("*|rect", "r"),
            ("*|li", "a"),
            ("|rect, |li, |*", ""),
            ("*|*[id=g]", "g"),
            ("[*|data-v]", "a"),
        ] {
            assert_eq!(query(selector), expected, "{selector}");
        }
        assert_eq!(Selector::try_parse("|rect").unwrap()[0].source(), "|rect");
        let doc =
            Document::parse("<style>LI{color:red}[DATA-V]{margin-top:1px}</style><li data-v></li>");
        let styles = crate::style::styles(&doc, 800., 600.);
        let li = doc.nodes.iter().position(|n| n.tag == "li").unwrap();
        assert_eq!(styles[li]["color"], "red");
        assert_eq!(styles[li]["margin-top"], "1px");
    }

    #[test]
    fn hostile_selectors_never_panic() {
        let alphabet: Vec<char> = "ab*|#.:()[]=~^$'\"\\>+, -_1nof&!é😀i".chars().collect();
        let doc =
            Document::parse("<div id=x class=a><p lang=en><b>t</b></p></div><svg><rect/></svg>");
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..2000 {
            let mut text = String::new();
            for _ in 0..(seed % 24) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            for prefix in ["", ":is(", ":has(", ":not(", "[", ":nth-child("] {
                let source = format!("{prefix}{text}");
                for selectors in [
                    Selector::try_parse(&source).unwrap_or_default(),
                    Selector::parse(&source, None),
                ] {
                    for selector in &selectors {
                        selector.source();
                        for id in 0..doc.nodes.len() {
                            selector.matches(&doc, id);
                        }
                    }
                }
            }
        }
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
        let mut cache = crate::style::StyleCache::default();
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
            let cached =
                crate::style::compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
            let fresh = crate::style::compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut crate::style::StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "step {step}");
            for (index, (id, key, value)) in checks.into_iter().enumerate() {
                assert_eq!(
                    cached.values[id]
                        .get(crate::style::Prop::new(key))
                        .is_some_and(|v| v == value),
                    expected.contains(&index),
                    "step {step}: {key}:{value}"
                );
            }
        }
    }

    #[test]
    fn slotted_and_part_take_what_chrome_takes_after_them() {
        let element = Some(usize::MAX);
        for (selector, target) in [
            ("::part(a)", element),
            ("x::part(a)::before", Some(0)),
            ("::part(a)::selection", Some(2)),
            ("::part(a)::marker", Some(3)),
            ("::part(a)::first-line", Some(5)),
            ("::part(a)::placeholder", Some(6)),
            ("::part(a)::-webkit-scrollbar-thumb:hover", Some(11)),
            ("::part(a)::backdrop", Some(15)),
            ("::part(a):hover:focus", element),
            ("::part(a):not(:hover)", element),
            ("::part(a):state(x)", element),
            ("::part(a):hover::after", Some(1)),
            ("::part(a):first-child", None),
            ("::part(a):not(:first-child)", None),
            ("::part(a):empty", None),
            ("::part(a):has(b)", None),
            ("::part(a):root", None),
            ("::part(a):is(.x)", None),
            ("::part(a).x", None),
            ("::part(a)::before:hover", None),
            ("::part(a)::part(b)", None),
            ("::part(a)::slotted(b)", None),
            ("::slotted(a)", element),
            ("::slotted(a)::before", Some(0)),
            ("::slotted(a)::after", Some(1)),
            ("::slotted(a)::marker", Some(3)),
            ("::slotted(a)::placeholder", Some(6)),
            ("::slotted(a)::backdrop", Some(15)),
            ("::slotted(a)::starting-style", Some(14)),
            ("::part(a)::backdrop::starting-style", Some(16)),
            ("::slotted(a)::first-line", None),
            ("::slotted(a)::selection", None),
            ("::slotted(a)::-webkit-scrollbar", None),
            ("::slotted(a):hover", None),
            ("::slotted(a)::part(b)", None),
        ] {
            let parsed = Selector::parse(selector, None);
            assert_eq!(
                parsed[0].shadow_target().map(|(kind, _)| kind),
                target,
                "{selector}"
            );
        }
    }
}
