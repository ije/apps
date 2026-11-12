use super::Document;
use serde::{Deserialize, Serialize};
use std::{
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
    #[serde(skip)]
    ancestry: std::cell::OnceCell<Option<Ancestry>>,
}

const BLOOM: usize = 4;
type Bloom = [u64; BLOOM];
pub(crate) type Ancestry = (Bloom, Box<[u16]>, u16);

// Lowercase tags and attributes on both sides, or the filter rejects valid matches.
fn feature(bloom: &mut Bloom, kind: u8, value: &str) {
    let hash = rustc_hash::FxBuildHasher.hash_one((kind, value));
    for shift in [0, 32] {
        let bit = (hash >> shift) as usize % (64 * BLOOM);
        bloom[bit / 64] |= 1 << (bit % 64);
    }
}

fn lowercase(value: &str) -> std::borrow::Cow<'_, str> {
    match value.bytes().any(|c| c.is_ascii_uppercase()) {
        true => value.to_ascii_lowercase().into(),
        false => value.into(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
enum SimpleSelector {
    Tag(String),
    Id(String),
    Class(String),
    Universal,
    Root,
    // The flag is 1 for `i`, and 2 for the HTML attributes whose values ignore case on HTML elements.
    Attribute(String, String, String, u8),
    Pseudo(String, String, #[serde(skip)] Rc<[Selector]>, [i32; 2]),
    Element(String, String),
    Parent(#[serde(skip)] SelectorList, usize),
    NoNamespace,
}

const PSEUDOS: [&str; 4] = [
    "active active-view-transition any-link autofill blank buffering checked closed corner-present decrement default defined disabled double-button empty enabled end first-child first-of-type focus focus-visible focus-within fullscreen future has-slotted horizontal host hover in-range increment indeterminate invalid last-child last-of-type link local-link modal muted no-button only-child only-of-type open optional out-of-range past paused picture-in-picture placeholder-shown playing popover-open read-only read-write required root scope seeking single-button stalled start target target-within user-invalid user-valid valid vertical visited volume-locked window-inactive -webkit-any-link -webkit-autofill -webkit-full-screen",
    "active-view-transition-type current dir has host host-context is lang not nth-child nth-last-child nth-last-of-type nth-of-type state where -webkit-any",
    "after backdrop before checkmark column cue details-content file-selector-button first-letter first-line grammar-error marker picker-icon placeholder scroll-marker scroll-marker-group search-text selection spelling-error starting-style target-text view-transition",
    "cue highlight part picker scroll-button slotted view-transition-group view-transition-image-pair view-transition-new view-transition-old",
];

// Their values match without case on HTML elements, from the HTML standard.
const HTML_INSENSITIVE: &str = "accept accept-charset align alink axis bgcolor charset checked clear codetype color compact declare defer dir direction disabled enctype face frame hreflang http-equiv lang language link media method multiple nohref noresize noshade nowrap readonly rel rev rules scope scrolling selected shape target text type valign valuetype vlink";

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

// The place of each element child, and the element children in order.
type Positions = Rc<(rustc_hash::FxHashMap<usize, [usize; 4]>, Vec<usize>)>;

type NthPositions = Rc<(rustc_hash::FxHashMap<usize, usize>, Vec<Dependency>)>;

// The arguments of a `:has()`, its result, and whether the arguments read siblings.
type Found = (Rc<[Selector]>, bool, bool);

pub(crate) type Dependency = (u32, u32);

#[derive(Clone, Default)]
struct Snapshot {
    values: [u32; KINDS],
    revisions: [u32; KINDS],
}
const KINDS: usize = 13;
// The place of an element among its siblings. These kinds are kept only for the children of a node with
// the kind's bit in `watched`, so read them through `placed`.
// 1: the first element child. 2: the last one.
const EDGE: usize = 9;
// The element sibling before the node, plus 1.
const PREVIOUS: usize = 10;
// The count of element siblings before the node.
const INDEX: usize = 11;
// Changes when a `:has()` result in `within` flips for the node.
const WITHIN: usize = 12;
// `watched` bits after the three place kinds: children hold `:has()` results that read their siblings, and
// the node holds results of its own.
const BESIDE: u8 = 8;
const BELOW: u8 = 16;

#[derive(Default)]
pub(crate) struct Named {
    names: rustc_hash::FxHashSet<String>,
    valued: rustc_hash::FxHashSet<String>,
    classes: rustc_hash::FxHashSet<u32>,
    // Counts the new names. A name changes kinds 0 and 8 of nodes that were hashed without it.
    generation: u32,
}

#[derive(Default)]
pub(crate) struct SelectorCache {
    pub(crate) attributes: Named,
    nodes: Vec<Snapshot>,
    seen: Vec<u64>,
    // The walk and document revision of the last update, and the nodes forgotten since.
    walked: u64,
    latest: u64,
    forgotten: Vec<usize>,
    values: Matches,
    size: usize,
    positions: rustc_hash::FxHashMap<usize, (u32, Positions)>,
    nth: rustc_hash::FxHashMap<(usize, usize, Option<usize>), NthPositions>,
    pub(crate) dependencies: Vec<Dependency>,
    revision: u32,
    pub(crate) epoch: u64,
    changes: std::collections::VecDeque<(u64, Vec<u32>)>,
    // An incremental pass styles only `dirty` nodes, so every recorded dependency must reach `dependents`.
    pub(crate) dirty: Vec<bool>,
    dependents: Vec<(u32, Vec<u32>)>,
    children: Vec<(u32, Vec<u32>)>,
    hosts: Rc<rustc_hash::FxHashMap<usize, usize>>,
    blooms: Vec<(Bloom, u32)>,
    pass: u32,
    chain: Vec<usize>,
    stamps: Vec<u32>,
    stamp: u32,
    named: u32,
    watched: Vec<u8>,
    // The `:has()` results of each anchor that read only below it or its parent. `update` checks them again,
    // so a match reads them without a scan.
    within: rustc_hash::FxHashMap<usize, Vec<Found>>,
}

impl SelectorCache {
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

    // Clear selector results when scopes change: selector addresses key the cache.
    pub(crate) fn forget_selectors(&mut self) {
        self.values.clear();
        self.size = 0;
        self.within.retain(|_, results| {
            results.retain(|result| Rc::strong_count(&result.0) > 1);
            !results.is_empty()
        });
        let mut changes = Vec::new();
        for ((_, parent, _), _) in std::mem::take(&mut self.nth) {
            if let Some(slot) = self.nodes.get_mut(parent) {
                self.revision = self.revision.wrapping_add(1);
                slot.revisions[2] = self.revision;
                changes.push((parent * KINDS + 2) as u32);
            }
        }
        self.record(changes, Vec::new());
    }

    fn record(&mut self, mut changes: Vec<u32>, quiet: Vec<u32>) {
        for &change in &changes {
            let place = matches!(change as usize % KINDS, 2 | EDGE..=INDEX);
            self.mark(change as usize / KINDS, place);
        }
        for &change in &quiet {
            if let Some(dirty) = self.dirty.get_mut(change as usize / KINDS) {
                *dirty = true;
            }
        }
        changes.extend(quiet);
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
            self.forgotten.push(id);
            self.mark(id, false);
            // Clear the lists of a forgotten node, or a node that reuses its id marks them again.
            self.dependents[id] = Default::default();
            self.children[id] = Default::default();
            self.within.remove(&id);
        }
    }

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

    // `dependencies` must be sorted, as `chunk_by` groups them by node.
    pub(crate) fn register(&mut self, id: usize) {
        for group in self
            .dependencies
            .chunk_by(|a, b| a.0 as usize / KINDS == b.0 as usize / KINDS)
        {
            let node = group[0].0 as usize / KINDS;
            let place = |&(kind, _): &Dependency| matches!(kind as usize % KINDS, 2 | EDGE..=INDEX);
            let lists = if group.iter().any(place) {
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
                    // Drop forgotten nodes during compaction, or parents retain dependencies on every past
                    // child.
                    self.stamp = self.stamp.wrapping_add(1);
                    if self.stamp == 0 {
                        self.stamps.fill(0);
                        self.stamp = 1;
                    }
                    if self.stamps.len() < self.seen.len() {
                        self.stamps.resize(self.seen.len(), 0);
                    }
                    let (seen, stamps, stamp) = (&self.seen, &mut self.stamps, self.stamp);
                    list.retain(|&node| {
                        seen.get(node as usize).is_some_and(|&seen| seen != 0)
                            && std::mem::replace(&mut stamps[node as usize], stamp) != stamp
                    });
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
        self.watched.truncate(len);
        self.watched.shrink_to_fit();
        self.within.retain(|&anchor, _| anchor < len);
    }

    pub(crate) fn update(&mut self, doc: &Document, walk: &super::Walk) {
        self.pass = self.pass.wrapping_add(1).max(1);
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
        self.watched
            .resize(doc.nodes.len().max(self.watched.len()), 0);
        let renamed = self.named != self.attributes.generation;
        self.named = self.attributes.generation;
        let mut changes = Vec::new();
        let mut quieted = Vec::new();
        let mut retagged = Vec::new();
        let mut moved = Vec::new();
        let start = self.revision;
        // Since the last update, only a node with a newer revision, a forgotten one or one that a patched walk
        // touched can have another snapshot. Debug builds check all nodes against that.
        let touched = match self.walked == walk.serial {
            true => Some(&[][..]),
            false => walk.since(self.walked),
        };
        let listed: Option<Vec<usize>> = touched.map(|touched| {
            let newer = doc.nodes.changed_since(self.latest);
            let touched = touched.iter().map(|&id| id as usize);
            let nodes = newer.chain(touched).chain(self.forgotten.iter().copied());
            nodes.filter(|&id| walk.contains(id)).collect()
        });
        (self.walked, self.latest) = (walk.serial, doc.nodes.latest());
        self.forgotten.clear();
        let mut every = walk.nodes.iter().map(|node| node.0 as usize);
        let mut some = listed.iter().flatten().copied();
        let nodes: &mut dyn Iterator<Item = usize> = match &listed {
            Some(_) if !cfg!(debug_assertions) && !renamed => &mut some,
            _ => &mut every,
        };
        for id in nodes {
            let revision = doc.nodes.revision(id);
            let fresh = self.seen[id] != revision;
            if !fresh && !renamed && !cfg!(debug_assertions) {
                continue;
            }
            debug_assert!(
                !fresh || listed.as_ref().is_none_or(|listed| listed.contains(&id)),
                "node {id} changed outside the list"
            );
            self.seen[id] = revision;
            let node = &doc.nodes[id];
            let Snapshot { values, revisions } = &mut self.nodes[id];
            let hash = rustc_hash::FxBuildHasher;
            let tag = hash.hash_one(&node.tag);
            let attributes = node
                .attrs
                .iter()
                // These attributes count only when selectors name them, so no pseudo-class may read them.
                .filter_map(|(key, value)| {
                    let selected = key.starts_with("data-")
                        || key.starts_with("aria-")
                        || matches!(key.as_str(), "key" | "title" | "role" | "tabindex");
                    (key.as_str() != "style"
                        && (key.as_str() != "class" || self.attributes.names.contains("class"))
                        && (!selected || self.attributes.names.contains(key.as_str())))
                    .then(|| {
                        (
                            key,
                            (!selected || self.attributes.valued.contains(key.as_str()))
                                .then_some(value),
                        )
                    })
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
                node.attrs
                    .get("class")
                    .map_or(0, |classes| hash.hash_one(classes) as u32),
                match node.attrs.get("class") {
                    Some(classes) if !self.attributes.classes.is_empty() => classes
                        .split_ascii_whitespace()
                        .map(|class| hash.hash_one(class) as u32)
                        .filter(|class| self.attributes.classes.contains(class))
                        .fold(0u32, |sum, class| {
                            sum.wrapping_add(class.wrapping_mul(0x9e37_79b9) | 1)
                        }),
                    _ => 0,
                },
            ];
            debug_assert!(
                fresh || new[1..8] == values[1..8],
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
                    match kind {
                        // A result that read the siblings holds for the old parent only. Its readers
                        // depend on the parent too.
                        1 => {
                            moved.extend(node.parent);
                            if let Some(results) = self.within.get_mut(&id) {
                                results.retain(|result| !result.2);
                                if results.is_empty() {
                                    self.within.remove(&id);
                                }
                            }
                        }
                        2 => moved.push(id),
                        _ => {}
                    }
                    if kind == 7 {
                        quieted.push((id * KINDS + kind) as u32);
                    } else {
                        changes.push((id * KINDS + kind) as u32);
                    }
                }
            }
        }
        // Retag each parent once per update to avoid revisiting its dependents for every child.
        for parent in retagged {
            if let Some(slot) = self.nodes.get_mut(parent)
                && slot.revisions[2].wrapping_sub(start).wrapping_sub(1)
                    >= self.revision.wrapping_sub(start)
            {
                self.revision = self.revision.wrapping_add(1);
                slot.revisions[2] = self.revision;
                changes.push((parent * KINDS + 2) as u32);
            }
        }
        moved.sort_unstable();
        moved.dedup();
        for parent in moved {
            self.place(doc, parent, 0, &mut changes);
        }
        let mut checked = 0;
        // The second round drops the positions that read a flipped `:has()` result.
        for round in 0..2 {
            // Dropping cached `:nth-child(An+B of S)` positions must count as a children change of their
            // parent.
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
                            fresh.iter().any(|id| {
                                dependencies.binary_search_by_key(id, |&(id, _)| id).is_ok()
                            })
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
            if round == 1 || self.within.is_empty() {
                break;
            }
            // A change can flip the `:has()` results of the nodes above it, and of their children that read
            // siblings.
            self.stamp = self.stamp.wrapping_add(1);
            if self.stamp == 0 {
                self.stamps.fill(0);
                self.stamp = 1;
            }
            if self.stamps.len() < self.seen.len() {
                self.stamps.resize(self.seen.len(), 0);
            }
            let mut above = Vec::new();
            // A local result reads no style attribute or own class list, and a new place follows a change
            // of the parent's children.
            let read = |change: &&u32| !matches!(**change as usize % KINDS, 4 | 6 | 7 | EDGE..);
            let mut kinds = 0;
            for &change in changes.iter().filter(read) {
                kinds |= 1 << (change as usize % KINDS);
                let mut at = Some(change as usize / KINDS);
                while let Some(node) = at.filter(|&node| {
                    std::mem::replace(&mut self.stamps[node], self.stamp) != self.stamp
                }) {
                    if self.watched[node] & (BESIDE | BELOW) != 0 {
                        above.push(node);
                    }
                    at = doc.nodes[node].parent;
                }
            }
            let start = self.dependencies.len();
            for trigger in above {
                let beside = self.watched[trigger] & BESIDE != 0;
                let children = doc.nodes[trigger].children.iter().filter(|_| beside);
                for (index, anchor) in std::iter::once(&trigger).chain(children).enumerate() {
                    let Some(mut results) = self.within.remove(anchor) else {
                        continue;
                    };
                    results.retain_mut(|(parsed, found, sibling)| {
                        let reads = parsed
                            .iter()
                            .fold(6, |all, selector| all | selector.reads());
                        if *sibling != (index > 0) || kinds & reads == 0 {
                            return true;
                        }
                        // Only this list holds the arguments of a dropped rule.
                        let live = Rc::strong_count(parsed) > 1;
                        let now = live && Selector::has(parsed, doc, *anchor, self, true).0;
                        if now != *found || !live {
                            *found = now;
                            self.revision = self.revision.wrapping_add(1);
                            self.nodes[*anchor].revisions[WITHIN] = self.revision;
                            changes.push((anchor * KINDS + WITHIN) as u32);
                        }
                        live
                    });
                    if !results.is_empty() {
                        self.within.insert(*anchor, results);
                    }
                }
            }
            self.dependencies.truncate(start);
        }
        self.record(changes, quieted);
    }

    // Set the place kinds that `parent` watches on its element children, after it starts to watch `first`.
    fn place(&mut self, doc: &Document, parent: usize, first: u8, changes: &mut Vec<u32>) {
        let Some(watched) = self.watched.get_mut(parent) else {
            return;
        };
        *watched |= first;
        let watched = *watched;
        if watched & 7 == 0 {
            return;
        }
        let children = &doc.nodes[parent].children;
        let element = |child: &&usize| !doc.nodes[**child].tag.starts_with('#');
        let last = children.iter().rfind(element);
        let (mut previous, mut index) = (0, 0);
        for child in children.iter().filter(element) {
            let new = [
                u32::from(previous == 0) | u32::from(Some(child) == last) << 1,
                previous,
                index,
            ];
            (previous, index) = (*child as u32 + 1, index + 1);
            let Some(Snapshot { values, revisions }) = self.nodes.get_mut(*child) else {
                continue;
            };
            for (kind, new) in (EDGE..).zip(new) {
                if watched >> (kind - EDGE) & 1 == 1 && values[kind] != new {
                    values[kind] = new;
                    self.revision = self.revision.wrapping_add(1);
                    revisions[kind] = self.revision;
                    changes.push((child * KINDS + kind) as u32);
                }
            }
        }
    }

    // Depend on a place kind of `id`. The first read below a parent sets the kind on all its children.
    fn placed(&mut self, doc: &Document, id: usize, kind: usize) {
        let bit = 1 << (kind - EDGE);
        if let Some(parent) = doc.nodes[id].parent
            && self
                .watched
                .get(parent)
                .is_some_and(|watched| watched & bit == 0)
        {
            // No match depends on the new values yet.
            self.place(doc, parent, bit, &mut Vec::new());
        }
        self.depend(id, kind);
    }

    fn positions(&mut self, doc: &Document, parent: usize) -> Positions {
        let revision = self.nodes.get(parent).map_or(0, |node| node.revisions[2]);
        if let Some((cached, positions)) = self.positions.get(&parent)
            && *cached == revision
        {
            return positions.clone();
        }
        let mut types = rustc_hash::FxHashMap::<&str, usize>::default();
        let mut places = rustc_hash::FxHashMap::default();
        let mut order = Vec::new();
        for &child in &doc.nodes[parent].children {
            let tag = doc.nodes[child].tag.as_str();
            if !tag.starts_with('#') {
                let of_type = types.entry(tag).or_default();
                places.insert(child, [order.len(), 0, *of_type, 0]);
                order.push(child);
                *of_type += 1;
            }
        }
        for (child, place) in &mut places {
            place[1] = order.len();
            place[3] = types[doc.nodes[*child].tag.as_str()];
        }
        let positions = Rc::new((places, order));
        self.positions.insert(parent, (revision, positions.clone()));
        positions
    }

    pub(crate) fn ancestors(&mut self, doc: &Document, id: usize) -> Bloom {
        if self.blooms.len() < doc.nodes.len() {
            self.blooms.resize(doc.nodes.len(), ([0; BLOOM], 0));
        }
        let mut bloom = [0; BLOOM];
        let mut node = doc.nodes[id].parent;
        while let Some(at) = node {
            if self.blooms[at].1 == self.pass {
                bloom = self.blooms[at].0;
                break;
            }
            self.chain.push(at);
            node = doc.nodes[at].parent;
        }
        while let Some(at) = self.chain.pop() {
            let node = &doc.nodes[at];
            if !node.tag.starts_with('#') {
                feature(&mut bloom, 0, &lowercase(&node.tag));
                for (name, value) in node.attrs.iter() {
                    match name.as_str() {
                        "id" => feature(&mut bloom, 1, value),
                        "class" => {
                            for class in value.split_ascii_whitespace() {
                                feature(&mut bloom, 2, class);
                            }
                        }
                        _ => {}
                    }
                    feature(&mut bloom, 3, &lowercase(name));
                }
            }
            self.blooms[at] = (bloom, self.pass);
        }
        bloom
    }

    pub(crate) fn reject(
        &mut self,
        doc: &Document,
        id: usize,
        (_, steps, _): &Ancestry,
        rest: u16,
    ) {
        let depend = |this: &mut Self, at: usize, kinds: u16| {
            for kind in 0..KINDS {
                if kinds >> kind & 1 == 1 {
                    this.depend(at, kind);
                }
            }
        };
        self.depend(id, 1);
        let mut node = doc.nodes[id].parent;
        for &kinds in steps.iter() {
            let Some(at) = node else { return };
            depend(self, at, kinds);
            node = doc.nodes[at].parent;
        }
        while let Some(at) = node.filter(|_| rest != 0) {
            depend(self, at, rest);
            node = doc.nodes[at].parent;
        }
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
            up = doc.nodes[node]
                .parent
                .or_else(|| self.hosts.get(&node).copied());
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

    pub(crate) fn share(
        &mut self,
        doc: &Document,
        from: usize,
        to: usize,
        dependencies: &[Dependency],
    ) -> bool {
        let mut pairs = [(0, 0); 16];
        let (mut length, mut up) = (0, (Some(from), Some(to)));
        let top = loop {
            match up {
                (Some(a), Some(b)) if a == b => break a,
                (Some(a), Some(b)) if length < pairs.len() => {
                    pairs[length] = (a, b);
                    length += 1;
                    let parent = |node: usize| {
                        doc.nodes[node]
                            .parent
                            .or_else(|| self.hosts.get(&node).copied())
                    };
                    up = (parent(a), parent(b));
                }
                _ => return false,
            }
        };
        let pairs = &pairs[..length];
        self.dependencies.clear();
        for &(read, revision) in dependencies {
            let (node, kind) = (read as usize / KINDS, read as usize % KINDS);
            let dependency = match pairs.iter().find(|pair| pair.0 == node) {
                Some(&(_, other)) => {
                    // Each node has its own `:has()` results.
                    if kind == WITHIN {
                        return false;
                    }
                    // The places below two parents compare when both keep the kind. The parent of
                    // `node` does.
                    if matches!(kind, EDGE..=INDEX) {
                        let (Some(parent), bit) = (doc.nodes[other].parent, 1 << (kind - EDGE))
                        else {
                            return false;
                        };
                        if self
                            .watched
                            .get(parent)
                            .is_none_or(|watched| watched & bit == 0)
                        {
                            self.place(doc, parent, bit, &mut Vec::new());
                        }
                    }
                    if !(kind == 1 || kind == 6 && node == from)
                        && self.nodes[node].values[kind] != self.nodes[other].values[kind]
                    {
                        return false;
                    }
                    (
                        (other * KINDS + kind) as u32,
                        self.nodes[other].revisions[kind],
                    )
                }
                None if node == top && kind == 2 => return false,
                None => (read, revision),
            };
            self.dependencies.push(dependency);
        }
        self.dependencies.sort_unstable();
        true
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

// Match `disabled` in dom.js: a disabled fieldset disables the controls in it, except in its first legend,
// and a disabled optgroup its options.
fn disabled(doc: &Document, id: usize, memo: &mut SelectorCache) -> bool {
    let tag = doc.nodes[id].tag.as_str();
    if !matches!(
        tag,
        "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
    ) {
        return false;
    }
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
fn submits(node: &super::Node) -> bool {
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

impl Selector {
    fn argument(
        name: &str,
        argument: &str,
        parent: Option<&SelectorList>,
        strict: bool,
    ) -> Option<(Vec<Self>, [i32; 2])> {
        let list = |source: &str| {
            Self::list(source, parent, strict, false).filter(|list| {
                !list.is_empty()
                    && !list.iter().any(|selector| {
                        let mut simple = selector.simple.iter();
                        simple.any(|simple| matches!(simple, SimpleSelector::Element(..)))
                    })
            })
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
                // Nested `:has()` is invalid and makes matching cost O(N²) or more.
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
                        *parsed = selectors.into();
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

    pub(crate) fn attributes(&self, names: &mut Named, subject: bool) {
        if let Some((_, previous)) = &self.previous {
            previous.attributes(names, false);
        }
        for simple in &self.simple {
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
                SimpleSelector::Pseudo(_, _, parsed, _) => {
                    for selector in parsed.iter() {
                        selector.attributes(names, false);
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
                    for selector in parsed.iter() {
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
        if kind == 15
            && matches!(&self.simple[index + 1..], [SimpleSelector::Element(name, _)]
                if name == "starting-style")
        {
            selector.simple.truncate(index);
            return Some((16, selector));
        }
        if matches!(kind, 12 | 13) {
            selector.simple.truncate(index);
            return Some((kind, selector));
        }
        if !(7..12).contains(&kind) {
            selector.simple.remove(index);
            return Some((kind, selector));
        }
        let kind = match (kind, &self.simple[index + 1..]) {
            (11, []) => 8,
            (_, []) => kind,
            (9, [SimpleSelector::Pseudo(name, ..)]) if name == "hover" => 11,
            _ => return None,
        };
        selector.simple.truncate(index);
        Some((kind, selector))
    }

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
            ancestry: Default::default(),
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

    // Match matches_with's ancestor dependencies: classes (8), otherwise tag and attributes (0), plus style
    // (6).
    pub(crate) fn ancestry(&self) -> Option<&Ancestry> {
        self.ancestry
            .get_or_init(|| {
                let (mut bloom, mut steps, mut rest) = ([0; BLOOM], Vec::new(), 0u16);
                let mut link = &self.previous;
                while let Some((combinator, previous)) = link {
                    if !matches!(combinator, ' ' | '>') {
                        return None;
                    }
                    let mut kinds = 1 << 1;
                    let mut classes = false;
                    for simple in &previous.simple {
                        match simple {
                            SimpleSelector::Tag(name) => feature(&mut bloom, 0, &lowercase(name)),
                            SimpleSelector::Id(id) => feature(&mut bloom, 1, id),
                            SimpleSelector::Class(class) => {
                                classes = true;
                                feature(&mut bloom, 2, class);
                            }
                            SimpleSelector::Attribute(name, ..) => {
                                if name.eq_ignore_ascii_case("style") {
                                    kinds |= 1 << 6;
                                }
                                feature(&mut bloom, 3, &lowercase(name));
                            }
                            SimpleSelector::Universal => {}
                            _ => return None,
                        }
                    }
                    if !classes
                        || previous.simple.iter().any(|simple| {
                            !matches!(simple, SimpleSelector::Class(_) | SimpleSelector::Universal)
                        })
                    {
                        kinds |= 1;
                    }
                    if classes {
                        kinds |= 1 << 8;
                    }
                    if *combinator == '>' && rest == 0 {
                        steps.push(kinds);
                    } else {
                        rest |= kinds;
                    }
                    link = &previous.previous;
                }
                (bloom != [0; BLOOM]).then(|| (bloom, steps.into(), rest))
            })
            .as_ref()
    }

    pub(crate) fn key(&self) -> Option<(u8, &str)> {
        self.simple
            .iter()
            .filter_map(|selector| match selector {
                SimpleSelector::Id(id) => Some((0, id.as_str())),
                SimpleSelector::Class(class) => Some((1, class.as_str())),
                // Keys compare exactly, so mixed-case names, which also match lowercase HTML names, must get
                // no key.
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
                            &selectors[..]
                        }
                        _ => return None,
                    };
                    // A second call for the first selector doubles the cost at each level of nesting.
                    let key = alternatives.first()?.key()?;
                    alternatives[1..]
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
            source.push(' ');
            if *combinator != ' ' {
                source.push(*combinator);
                source.push(' ');
            }
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
                    if *insensitive == 1 {
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
        Self::list(source, parent, true, true).unwrap_or_default()
    }

    pub(crate) fn try_parse(source: &str) -> anyhow::Result<Vec<Self>> {
        Self::list(source, None, true, true)
            .ok_or_else(|| anyhow::anyhow!("SyntaxError: '{source}' is not a valid selector"))
    }

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
            let (mut quote, mut escaped) = (None, false);
            let explicit = rest.chars().any(|c| {
                let plain = !escaped && quote.is_none();
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if quote == Some(c) {
                    quote = None;
                } else if plain && matches!(c, '"' | '\'') {
                    quote = Some(c);
                }
                plain && c == '&'
            });
            let mut previous = parent
                .filter(|_| nested && (!explicit || rest.starts_with(['>', '+', '~'])))
                .map(|parent| Self {
                    simple: vec![SimpleSelector::Parent(Rc::clone(parent), 1)],
                    previous: None,
                    specificity: parent_specificity,
                    ancestry: Default::default(),
                });
            let mut compounds = 0;
            while !rest.is_empty() {
                // Matching recurses per compound, so long chains can overflow the stack.
                compounds += 1;
                if compounds > 256 {
                    return Some(Vec::new());
                }
                let mut combinator = ' ';
                let before = previous
                    .as_ref()
                    .map_or(&[][..], |previous| &previous.simple);
                if before
                    .iter()
                    .any(|simple| matches!(simple, SimpleSelector::Element(..)))
                {
                    return None;
                }
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
                            ("", String::new(), 0)
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
                                "" if HTML_INSENSITIVE
                                    .split(' ')
                                    .any(|known| name.eq_ignore_ascii_case(known)) =>
                                {
                                    2
                                }
                                "" | "s" | "S" => 0,
                                "i" | "I" => 1,
                                _ => return None,
                            };
                            (operator, value, insensitive)
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
                                simple.push(SimpleSelector::Pseudo(
                                    "scope".into(),
                                    String::new(),
                                    Rc::new([]),
                                    [0, 0],
                                ));
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
                            && !name.starts_with("-internal-")
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
                            selectors.into(),
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
                    ancestry: Default::default(),
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

    pub(crate) fn matches_fresh(
        &self,
        doc: &Document,
        id: usize,
        memo: &mut SelectorCache,
    ) -> bool {
        self.matches_with(doc, id, None, memo, false, true)
    }

    // A shadow root is featureless, so callers must not skip matching for it with this.
    pub(crate) fn always(&self) -> bool {
        self.previous.is_none()
            && self
                .simple
                .iter()
                .all(|simple| matches!(simple, SimpleSelector::Universal))
    }

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
        self.matches_with(doc, id, scope, memo, true, true)
    }

    // Whether the node has the tag, id and classes of the compound.
    fn plain(&self, node: &super::Node) -> bool {
        self.simple.iter().all(|simple| match simple {
            SimpleSelector::Tag(tag) => {
                node.tag == *tag
                    || node.tag.eq_ignore_ascii_case(tag)
                        && !node.tag.bytes().any(|c| c.is_ascii_uppercase())
            }
            SimpleSelector::Id(id) => node.attrs.get("id") == Some(id),
            SimpleSelector::Class(class) => node
                .attrs
                .get("class")
                .is_some_and(|classes| classes.split_ascii_whitespace().any(|c| c == class)),
            _ => true,
        })
    }

    // The kinds that a match of this `:has()` argument can read from a node, besides its place in the tree.
    fn reads(&self) -> u16 {
        let above = self.previous.as_ref();
        let above = above.map_or(0, |(_, previous)| previous.reads());
        self.simple.iter().fold(above, |all, simple| {
            all | match simple {
                SimpleSelector::Class(_) => 1 << 8,
                SimpleSelector::Universal => 0,
                SimpleSelector::Pseudo(name, _, parsed, _)
                    if matches!(name.as_str(), "scope" | "is" | "where" | "not") =>
                {
                    parsed
                        .iter()
                        .fold(0, |all, selector| all | selector.reads())
                }
                _ => 1 | 1 << 3 | 1 << 5,
            }
        })
    }

    // Whether a match of this `:has()` argument reads only nodes below the anchor, or below its parent when
    // the argument starts at a sibling, and no kind that `update` leaves out.
    fn local(&self, anchor: &super::Node) -> bool {
        self.simple.iter().all(|simple| match simple {
            SimpleSelector::Attribute(name, ..) => !name.eq_ignore_ascii_case("style"),
            SimpleSelector::Pseudo(name, _, parsed, _) => {
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
                        | "default"
                ) && !(name == "checked" && matches!(anchor.tag.as_str(), "optgroup" | "option"))
                    && parsed
                        .iter()
                        .all(|selector| selector.previous.is_none() && selector.local(anchor))
            }
            SimpleSelector::Parent(..) => false,
            _ => true,
        }) && self
            .previous
            .as_ref()
            .is_none_or(|(_, previous)| previous.local(anchor))
    }

    // Whether a relative selector of a `:has()` matches below or after `id`, and whether the selectors read
    // its siblings. A local scan records no dependencies of the nodes that fail.
    fn has(
        parsed: &[Self],
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
                (!local || selector.plain(below))
                    && selector.matches_nested(doc, child, Some(id), memo)
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
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
    ) -> bool {
        self.matches_with(doc, id, scope, memo, true, false)
    }

    fn matches_with(
        &self,
        doc: &Document,
        id: usize,
        scope: Option<usize>,
        memo: &mut SelectorCache,
        store: bool,
        subject: bool,
    ) -> bool {
        let key = (self as *const Self as usize, id, scope, false);
        let compound = !store
            || self.previous.is_none()
                && self.simple.iter().all(|simple| match simple {
                    SimpleSelector::Pseudo(_, _, parsed, _) => parsed.is_empty(),
                    SimpleSelector::Parent(..) => false,
                    _ => true,
                });
        let start = memo.dependencies.len();
        let classes = self
            .simple
            .iter()
            .any(|simple| matches!(simple, SimpleSelector::Class(_)));
        if !classes
            || self.simple.iter().any(|simple| {
                !matches!(simple, SimpleSelector::Class(_) | SimpleSelector::Universal)
            })
        {
            memo.depend(id, 0);
        }
        if classes {
            memo.depend(id, if subject { 7 } else { 8 });
        }
        if self.simple.iter().any(|simple| {
            matches!(simple, SimpleSelector::Attribute(name, ..) if name.eq_ignore_ascii_case("style"))
        }) {
            memo.depend(id, 6);
        }
        let node = &doc.nodes[id];
        // Most candidates of a scan fail here, so it runs before the lookup and stores no result.
        if !self.plain(node) {
            return false;
        }
        if !compound
            && let Some((result, dependencies)) = memo.values.get(&key)
            && memo.valid(dependencies)
        {
            memo.dependencies.truncate(start);
            memo.dependencies.extend(dependencies.iter().copied());
            return *result;
        }
        let result = (|| {
            let host = node
                .tag
                .starts_with('#')
                .then(|| memo.hosts.get(&id).copied())
                .flatten();
            if let Some(host) = host {
                let mut hosted = false;
                for simple in &self.simple {
                    let matched = match simple {
                        SimpleSelector::Universal => continue,
                        SimpleSelector::Pseudo(name, _, parsed, _) if name == "host" => {
                            parsed.is_empty()
                                || parsed
                                    .iter()
                                    .any(|s| s.matches_nested(doc, host, scope, memo))
                        }
                        SimpleSelector::Pseudo(name, _, parsed, _)
                            if matches!(name.as_str(), "is" | "where") =>
                        {
                            parsed
                                .iter()
                                .any(|s| s.matches_nested(doc, id, scope, memo))
                        }
                        SimpleSelector::Parent(parent, _) => parent
                            .iter()
                            .any(|s| s.matches_nested(doc, id, scope, memo)),
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
                    SimpleSelector::Tag(_) | SimpleSelector::Id(_) | SimpleSelector::Class(_) => {
                        true
                    }
                    SimpleSelector::Universal => true,
                    SimpleSelector::Root => node.tag == "html",
                    SimpleSelector::Attribute(name, operator, value, flag) => node
                        .attrs
                        .iter()
                        .find(|(key, _)| {
                            *key == name
                                || key.eq_ignore_ascii_case(name)
                                    && !key.bytes().any(|c| c.is_ascii_uppercase())
                        })
                        .is_some_and(|(_, actual)| {
                            let insensitive = *flag == 1
                                || *flag == 2 && !node.tag.bytes().any(|c| c.is_ascii_uppercase());
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
                    SimpleSelector::Pseudo(name, argument, parsed, [a, b]) => match name.as_str() {
                        "scope" => scope.map_or(node.tag == "html", |scope| {
                            scope == id
                                || node.parent == Some(scope) && doc.nodes[scope].tag == "#document"
                        }),
                        "has" => {
                            let local = memo.nodes.get(id).is_some()
                                && parsed.iter().all(|selector| selector.local(node));
                            if !local {
                                return Self::has(parsed, doc, id, memo, false).0;
                            }
                            let known = memo.within.get(&id).and_then(|results| {
                                results.iter().find(|result| Rc::ptr_eq(&result.0, parsed))
                            });
                            let (found, beside) = match known {
                                Some(result) => (result.1, result.2),
                                None => {
                                    let start = memo.dependencies.len();
                                    let (found, beside) = Self::has(parsed, doc, id, memo, true);
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
                        "defined" => {
                            memo.depend(id, 5);
                            node.state & super::DEFINED != 0
                                || !node.tag.contains('-')
                                || node.tag.starts_with("sf-symbol-")
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
                            matches!(node.tag.as_str(), "details" | "dialog")
                                && node.attrs.contains_key("open")
                        }
                        "indeterminate" => {
                            memo.depend(id, 5);
                            node.tag == "progress" && !node.attrs.contains_key("value")
                                || node.tag == "input"
                                    && node.state & super::INDETERMINATE != 0
                                    && node.input_type().eq_ignore_ascii_case("checkbox")
                        }
                        "default" => {
                            let checkable = node.tag == "input"
                                && ["checkbox", "radio"]
                                    .iter()
                                    .any(|kind| node.input_type().eq_ignore_ascii_case(kind));
                            if checkable || node.tag == "option" {
                                node.attrs.contains_key(if checkable {
                                    "checked"
                                } else {
                                    "selected"
                                })
                            } else if submits(node) {
                                let mut form = Some(id);
                                while let Some(at) = form.filter(|&at| doc.nodes[at].tag != "form")
                                {
                                    memo.depend(at, 1);
                                    form = doc.nodes[at].parent;
                                    form.inspect(|&form| memo.depend(form, 4));
                                }
                                let mut pending = Vec::from_iter(form);
                                let mut first = None;
                                while let Some(at) = pending.pop() {
                                    if doc.nodes[at].tag.starts_with('#') {
                                        continue;
                                    }
                                    memo.depend(at, 0);
                                    if Some(at) != form && submits(&doc.nodes[at]) {
                                        first = Some(at);
                                        break;
                                    }
                                    memo.depend(at, 2);
                                    pending.extend(doc.nodes[at].children.iter().rev());
                                }
                                first == Some(id)
                            } else {
                                false
                            }
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
                            ) && disabled(doc, id, memo) == (name == "disabled")
                        }
                        "valid" | "invalid" => {
                            // Match `candidate` in dom.js, which also sets the validity bit.
                            memo.depend(id, 1);
                            memo.depend(id, 5);
                            let candidate = match node.tag.as_str() {
                                "input" => !["hidden", "reset", "button"]
                                    .iter()
                                    .any(|kind| node.input_type().eq_ignore_ascii_case(kind)),
                                "button" => submits(node),
                                "select" | "textarea" => true,
                                _ => false,
                            };
                            candidate
                                && !node.attrs.contains_key("readonly")
                                && node.parent.is_none_or(|parent| {
                                    memo.depend(parent, 4);
                                    !doc.nodes[parent].tag.starts_with("datalist")
                                })
                                && !disabled(doc, id, memo)
                                && (node.state & super::INVALID != 0) == (name == "invalid")
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
                                    _ => !required,
                                }
                        }
                        "read-write" | "read-only"
                            if matches!(node.tag.as_str(), "input" | "textarea") =>
                        {
                            let writable = !node.attrs.contains_key("readonly")
                                && !disabled(doc, id, memo)
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
                            .any(|s| s.matches_nested(doc, id, scope, memo)),
                        "is" | "where" => parsed
                            .iter()
                            .any(|s| s.matches_nested(doc, id, scope, memo)),
                        "first-child" | "last-child" | "only-child" | "nth-child"
                        | "nth-last-child" | "first-of-type" | "last-of-type" | "only-of-type"
                        | "nth-of-type" | "nth-last-of-type" => {
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
                                    let Some(&[index, count, of_type, type_count]) =
                                        positions.0.get(&id)
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
                                        .any(|s| s.matches_nested(doc, id, scope, memo))
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
                                                    && parsed.iter().any(|s| {
                                                        s.matches_nested(doc, child, scope, memo)
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
                        .any(|s| s.matches_nested(doc, id, scope, memo)),
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
                '>' => previous.matches_nested(doc, parent, scope, memo),
                '+' => {
                    memo.placed(doc, id, PREVIOUS);
                    let positions = memo.positions(doc, parent);
                    let index = positions
                        .0
                        .get(&id)
                        .and_then(|place| place[0].checked_sub(1));
                    index.is_some_and(|index| {
                        previous.matches_nested(doc, positions.1[index], scope, memo)
                    })
                }
                '~' => {
                    memo.depend(parent, 2);
                    let positions = memo.positions(doc, parent);
                    // Keep the first sibling that matches, like the positions of `:nth-child(An+B of S)`:
                    // a scan from each node costs O(N²) time and dependencies.
                    let key = (&**previous as *const Self as usize, parent, scope);
                    let first = match memo.nth.get(&key) {
                        Some(first) => first.clone(),
                        None => {
                            let start = memo.dependencies.len();
                            memo.depend(parent, 2);
                            let first = positions.1.iter().enumerate().find(|&(_, &sibling)| {
                                previous.matches_nested(doc, sibling, scope, memo)
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
                    let start = memo.dependencies.len();
                    let mut matched = false;
                    while let Some(id) = ancestor {
                        // Memoize only reads from the ancestor upward, or sibling lookups inherit each
                        // other's branch dependencies.
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
                        if previous.matches_nested(doc, id, scope, memo) {
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
    use std::collections::HashMap;

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
    fn retagged_children_record_their_parent_once() {
        let mut doc = Document::parse("<ul><li>a</li><li>b</li><li>c</li></ul>");
        let [ul, li] =
            ["ul", "li"].map(|tag| doc.nodes.iter().position(|node| node.tag == tag).unwrap());
        let mut walk = crate::dom::Walk::default();
        let parent = |cache: &SelectorCache| {
            cache.changes.back().map_or(0, |(_, changes)| {
                changes
                    .iter()
                    .filter(|&&change| change as usize == ul * KINDS + 2)
                    .count()
            })
        };
        let mut cache = SelectorCache::default();
        walk.update(&doc, &Default::default(), None);
        cache.update(&doc, &walk);
        assert_eq!(parent(&cache), 1);
        doc.nodes[li].tag = "p".into();
        walk.update(&doc, &Default::default(), None);
        cache.update(&doc, &walk);
        assert_eq!(parent(&cache), 1);
    }

    #[test]
    fn ancestor_filter_rejections_read_the_walk_kinds() {
        let doc = Document::parse("<div class=y><p><span class=x></span></p></div>");
        let x = doc
            .nodes
            .iter()
            .position(|node| node.tag == "span")
            .unwrap();
        let mut walk = crate::dom::Walk::default();
        walk.update(&doc, &Default::default(), None);
        let above: Vec<usize> =
            std::iter::successors(doc.nodes[x].parent, |&id| doc.nodes[id].parent).collect();
        let mut cache = SelectorCache::default();
        cache.update(&doc, &walk);
        let bloom = cache.ancestors(&doc, x);
        for (source, steps, rest) in [
            ("[data-k] > .x", &[&[0, 1][..]][..], &[][..]),
            ("#i > .y > .x", &[&[1, 8][..], &[0, 1]], &[]),
            (".a .x", &[], &[1, 8]),
            ("div.b .x", &[], &[0, 1, 8]),
            (".c > .d .x", &[], &[1, 8]),
            ("[style] .b > .x", &[&[1, 8][..]], &[0, 1, 6]),
        ] {
            let ancestry = Selector::parse(source, None)[0]
                .ancestry()
                .cloned()
                .unwrap();
            assert!(
                ancestry
                    .0
                    .iter()
                    .zip(&bloom)
                    .any(|(need, have)| need & !have != 0),
                "{source}"
            );
            cache.dependencies.clear();
            cache.reject(&doc, x, &ancestry, ancestry.2);
            let mut expected = vec![(x, 1)];
            for (index, &at) in above.iter().enumerate() {
                let kinds = steps.get(index).copied().unwrap_or(rest);
                expected.extend(kinds.iter().map(|&kind| (at, kind)));
            }
            let read: Vec<(usize, usize)> = cache
                .dependencies
                .iter()
                .map(|&(key, _)| (key as usize / KINDS, key as usize % KINDS))
                .collect();
            assert_eq!(read, expected, "{source}");
        }
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
            "[lang=EN-us]",
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
            "[lang=EN-us s]",
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
            ":unknown",
            "::-moz-selection",
            ":-moz-focusring",
            "li:foo(1)",
            "a::before b",
            "a::before > b",
            "p:not(::before)",
            ":has(::after)",
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
            "::-webkit-scrollbar",
            "::-webkit-scrollbar-thumb:vertical",
            ":-webkit-any-link",
            ":is(:has(a)):not(:has(b))",
            "dialog:-internal-dialog-in-top-layer",
            "a::starting-style",
            "select, ::picker(select)",
            "a::before:hover",
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
            "a::before b",
            "p:not(::before)",
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
            (":invalid", ""),
            ("input:valid", "t,v,cb"),
            ("textarea:valid", "ta"),
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
    fn form_states_follow_fieldsets_and_pushed_bits() {
        let mut doc = Document::parse(
            "<style>:disabled{color:red}:invalid{color:blue}:not(:defined){color:green}:indeterminate,:default{color:gray}</style><form><fieldset id=fs disabled><legend id=lg><input id=a></legend><input id=b><legend id=lg2><input id=c></legend></fieldset><select><optgroup id=og disabled><option id=o1>x</option></optgroup><option id=o2 selected>y</option></select><input id=cb type=CheckBox checked><input id=sub type=submit><button id=btn></button><x-el id=x></x-el><sf-symbol-star id=sf></sf-symbol-star><input id=t readonly></form><button id=out></button><p dir=RTL id=p></p><svg><feColorMatrix id=m type=Saturate></feColorMatrix></svg>",
        );
        let find = |doc: &Document, id: &str| {
            (0..doc.nodes.len())
                .find(|&node| doc.nodes[node].attrs.get("id").is_some_and(|v| v == id))
                .unwrap()
        };
        let [fs, a, b, c, cb, x] = ["fs", "a", "b", "c", "cb", "x"].map(|id| find(&doc, id));
        for (selector, expected) in [
            (":disabled", "fs,b,c,og,o1"),
            ("input:enabled", "a,cb,sub,t"),
            ("input:read-write", "a"),
            (":default", "o2,cb,sub"),
            (":valid", "a,cb,sub,btn,out"),
            (":invalid, :indeterminate", ""),
            (":not(:defined)", "x"),
            ("[type=checkbox], [dir=rtl], [type=saturate]", "cb,p"),
            ("[dir=rtl s], [type=Saturate]", "m"),
        ] {
            assert_eq!(matched(&doc, selector), expected, "{selector}");
        }
        doc.nodes[x].state = super::super::DEFINED;
        doc.nodes[cb].state = super::super::INDETERMINATE;
        doc.nodes[a].state = super::super::INVALID;
        doc.nodes[b].state = super::super::INVALID;
        for (selector, expected) in [
            (":not(:defined)", ""),
            (":indeterminate", "cb"),
            (":invalid", "a"),
            (":has(> legend > :invalid)", "fs"),
        ] {
            assert_eq!(matched(&doc, selector), expected, "{selector}");
        }
        let mut sheets = HashMap::new();
        let mut cache = crate::style::StyleCache::default();
        let legend = find(&doc, "lg2");
        for step in 0..5 {
            match step {
                1 => drop(doc.nodes[fs].attrs.remove("disabled")),
                2 => doc.nodes[fs].attrs.set("disabled", ""),
                3 => {
                    let children = &mut doc.nodes[fs].children;
                    let at = children.iter().position(|&kid| kid == legend).unwrap();
                    children.remove(at);
                    children.insert(0, legend);
                }
                4 => {
                    doc.nodes[a].state = 0;
                    drop(doc.nodes[fs].attrs.remove("disabled"));
                }
                _ => {}
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
            let color = |id: usize| {
                let color = cached.values[id].get(crate::style::Prop::new("color"));
                ["red", "blue"]
                    .into_iter()
                    .find(|&name| color.is_some_and(|value| value == name))
                    .unwrap_or("-")
            };
            let expected = [
                ["blue", "red", "red"],
                ["blue", "blue", "-"],
                ["blue", "red", "red"],
                ["red", "red", "-"],
                ["-", "blue", "-"],
            ];
            assert_eq!([a, b, c].map(color), expected[step], "step {step}");
        }
        let parent = Rc::new(Selector::parse(".a", None));
        let nested = Document::parse(
            "<div class=a><i id=in title='R&D'></i></div><i id=out title='R&D'></i>",
        );
        for (source, expected) in [
            ("[title=\"R&D\"]", "in"),
            (".R\\&D, i", "in"),
            (":is(&) i", "in"),
        ] {
            let selectors = Selector::parse(source, Some(&parent));
            let found: Vec<_> = (0..nested.nodes.len())
                .filter(|&id| {
                    selectors
                        .iter()
                        .any(|selector| selector.matches(&nested, id))
                })
                .filter_map(|id| nested.nodes[id].attrs.get("id").cloned())
                .collect();
            assert_eq!(found.join(","), expected, "{source}");
        }
        let div = nested
            .nodes
            .iter()
            .position(|node| node.tag == "div")
            .unwrap();
        let scoped = Selector::try_parse("& > i").unwrap();
        let inside = find(&nested, "in");
        assert!(scoped[0].matches_scope(&nested, inside, Some(div)));
        assert_eq!(scoped[0].specificity, [0, 0, 1]);
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
