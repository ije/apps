use crate::css::{
    Declarations, Declared, MediaQuery, Prop, Registry, Rule, Selector, SelectorList, Stylesheet,
};
use crate::dom::{Document, SelectorCache};
use crate::style::cascade::container::style_query;
use crate::style::cascade::memo::CachedNode;
use crate::style::cascade::rank::AUTHOR;
use crate::style::cascade::shadow::{PART, SLOTTED};
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::rc::Rc;
pub(crate) type Key = Box<[(u8, String)]>;

pub(crate) const PSEUDO: usize = 1 << (usize::BITS - 1);
pub(crate) const SELECTOR: usize = 32;
#[derive(Default)]
pub(crate) struct StyleScope {
    pub(crate) id: usize,
    pub(crate) used: u64,
    pub(crate) fullscreen: bool,
    pub(crate) sheets: Vec<Rc<Stylesheet>>,
    pub(crate) rules: Vec<(SelectorList, Rc<[Declared]>, Layer)>,
    pub(crate) layers: Vec<(String, Vec<u32>)>,
    pub(crate) pseudos: Vec<(usize, Selector, usize, Key)>,
    pub(crate) arguments: FxHashMap<usize, (Rc<str>, SelectorList, usize)>,
    pub(crate) elements: Vec<Vec<(usize, Key)>>,
    pub(crate) buckets: [FxHashMap<String, Vec<usize>>; 5],
    pub(crate) universal: Vec<usize>,
    pub(crate) containers: Vec<Option<ContainerQuery>>,
    pub(crate) blocks: Vec<Rc<MediaBlock>>,
    pub(crate) medias: Vec<Option<MediaStack>>,
    pub(crate) keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    pub(crate) properties: Registry,
    pub(crate) states: u16,
    pub(crate) nodes: Vec<Option<MatchedRules>>,
    pub(crate) lists: RuleLists,
    pub(crate) keys: Vec<u128>,
    pub(crate) block: Option<u64>,
    pub(crate) sequence: u64,
    pub(crate) named: bool,
}

// While rules are collected, `rank` is the index of the layer in `Collect.layers`, not its rank.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Layer {
    pub(crate) origin: u8,
    pub(crate) rank: u32,
}

pub(crate) const DELETED: u8 = u8::MAX;
pub(crate) const UNLAYERED: u32 = u32::MAX;

pub(crate) type Collected<'a> = (
    &'a SelectorList,
    &'a Declarations,
    Layer,
    Option<ContainerQuery>,
    Option<MediaStack>,
);

impl StyleScope {
    pub(crate) fn push(
        &mut self,
        rules: Vec<Collected>,
        tailwind: Option<&FxHashMap<usize, u64>>,
        attributes: &mut crate::dom::Named,
    ) {
        let mut lists = Vec::new();
        for (selectors, body, layer, container, media) in rules {
            let order = self.rules.len();
            let key = tailwind.and_then(|keys| keys.get(&(Rc::as_ptr(selectors) as usize)));
            self.keys.push(match key {
                Some(key) => {
                    u128::from(*self.block.get_or_insert(self.sequence)) << 64 | u128::from(*key)
                }
                None => {
                    self.sequence += 1;
                    u128::from(self.sequence) << 64
                }
            });
            let elements = selectors
                .iter()
                .enumerate()
                .filter_map(|(index, selector)| {
                    if let Some((kind, mut pseudo)) = selector.without_pseudo_element() {
                        if matches!(kind, SLOTTED | PART) {
                            let (target, classes) = selector.shadow_target()?;
                            let argument = selector.pseudo_argument().unwrap_or_default().trim();
                            let compound = Rc::new(if kind == SLOTTED {
                                Selector::parse(argument, None)
                            } else {
                                vec![classes]
                            });
                            if kind == SLOTTED
                                && let Some(own) =
                                    compound.iter().map(|selector| selector.specificity).max()
                            {
                                for (value, own) in pseudo.specificity.iter_mut().zip(own) {
                                    *value = value.saturating_add(own);
                                }
                            }
                            lists.push(compound.clone());
                            self.arguments
                                .insert(self.pseudos.len(), (argument.into(), compound, target));
                        }
                        let key: Key = pseudo
                            .keys()
                            .into_iter()
                            .map(|(kind, name)| (kind, name.into_owned()))
                            .collect();
                        let index = PSEUDO | self.pseudos.len();
                        for (kind, name) in &key {
                            self.buckets[*kind as usize]
                                .entry(name.clone())
                                .or_default()
                                .push(index);
                        }
                        if key.is_empty() {
                            self.universal.push(index);
                        }
                        self.pseudos.push((kind, pseudo, order, key));
                        return None;
                    }
                    let key: Key = selector
                        .keys()
                        .into_iter()
                        .map(|(kind, name)| (kind, name.into_owned()))
                        .collect();
                    let entry = order << SELECTOR | index;
                    for (kind, name) in &key {
                        self.buckets[*kind as usize]
                            .entry(name.clone())
                            .or_default()
                            .push(entry);
                    }
                    if key.is_empty() {
                        self.universal.push(entry);
                    }
                    Some((index, key))
                })
                .collect();
            self.elements.push(elements);
            self.named |= container.as_ref().is_some_and(|container| {
                container.iter().any(|(name, _, query)| {
                    name.is_some() && style_query(query, None, &mut Vec::new()).is_some()
                })
            });
            self.containers.push(container);
            self.medias.push(media);
            self.rules
                .push((selectors.clone(), body.resolved(false).into(), layer));
            lists.push(selectors.clone());
        }
        // Parent selectors of nested rules are queued once each, so `& &` chains stay linear.
        let (mut seen, mut parents, mut subject) =
            (std::collections::HashSet::new(), Vec::new(), true);
        while let Some(list) = lists.pop().or_else(|| {
            subject = false;
            parents.pop()
        }) {
            if seen.insert((Rc::as_ptr(&list), subject)) {
                for selector in list.iter() {
                    self.states |= selector.states(&mut parents);
                    selector.attributes(attributes, subject);
                }
            }
        }
    }

    // Add the rules of new Tailwind chunks. `None` means that the scope cannot take them. `Some(true)` means
    // that only the nodes marked dirty can match them.
    pub(crate) fn insert(
        &mut self,
        update: &crate::style::tailwind::Update,
        rules: &[Collected],
        blocks: &[Rc<MediaBlock>],
        doc: &Document,
        matches: &mut SelectorCache,
    ) -> Option<bool> {
        let generated = |key: &u128| Some((key >> 64) as u64) == self.block && *key as u64 != 0;
        let layer = self.rules[self.keys.iter().position(generated)?].2;
        if update
            .replaced
            .iter()
            .any(|old| self.sheets.iter().any(|sheet| Rc::ptr_eq(sheet, old)))
        {
            return None;
        }
        for (old, new) in &update.added {
            let index = self
                .sheets
                .iter()
                .position(|sheet| Rc::ptr_eq(sheet, old))?;
            self.sheets.splice(index..=index, new.iter().cloned());
        }
        let start = self.rules.len();
        let rules = rules
            .iter()
            .map(|(selectors, body, _, container, media)| {
                (*selectors, *body, layer, container.clone(), media.clone())
            })
            .collect();
        self.push(rules, Some(update.keys), &mut matches.attributes);
        self.blocks.extend(blocks.iter().cloned());
        Some(self.rematch(start, doc, &mut matches.dirty))
    }

    // Forget the matched rules of the nodes that the rules from `start` can match, and mark them dirty. False
    // means that the rules can also reach other nodes, so only a full pass finds them all.
    pub(crate) fn rematch(&mut self, start: usize, doc: &Document, dirty: &mut [bool]) -> bool {
        let added = self.pseudos.iter().filter(|pseudo| pseudo.2 >= start);
        let passed = added
            .clone()
            .any(|pseudo| matches!(pseudo.0, SLOTTED | PART));
        let keys = self.elements[start..].iter().flatten().map(|(_, key)| key);
        let mut names: [rustc_hash::FxHashSet<&str>; 5] = Default::default();
        for key in keys.chain(added.map(|pseudo| &pseudo.3)) {
            if key.is_empty() {
                self.nodes.clear();
                return false;
            }
            for (kind, name) in key {
                names[*kind as usize].insert(name);
            }
        }
        let [ids, classes, tags, attributes, values] = &names;
        let mut pair = String::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            let class = |name: &String| {
                name.split_ascii_whitespace()
                    .any(|class| classes.contains(class))
            };
            let lower = crate::css::lowercase;
            if (tags.contains(&*lower(&node.tag))
                || !attributes.is_empty()
                    && node
                        .attrs
                        .keys()
                        .any(|name| attributes.contains(&*lower(name)))
                || !values.is_empty()
                    && node.attrs.iter().any(|(name, value)| {
                        pair.clear();
                        pair.push_str(&lower(name));
                        pair.push('=');
                        pair.push_str(value);
                        values.contains(pair.as_str())
                    })
                || !classes.is_empty() && node.attrs.get("class").is_some_and(class)
                || !ids.is_empty()
                    && node
                        .attrs
                        .get("id")
                        .is_some_and(|id| ids.contains(id.as_str())))
                && let Some(matched) = self.nodes.get_mut(id)
            {
                *matched = None;
                if let Some(dirty) = dirty.get_mut(id) {
                    *dirty = true;
                }
            }
        }
        !passed
    }
}

pub(crate) type RuleLists = rustc_hash::FxHashSet<Rc<[(usize, [usize; 3])]>>;

pub(crate) fn intern(
    lists: &mut RuleLists,
    list: &[(usize, [usize; 3])],
) -> Rc<[(usize, [usize; 3])]> {
    if let Some(old) = lists.get(list) {
        return old.clone();
    }
    if lists.len() > 65536 {
        lists.clear();
    }
    let list: Rc<[_]> = list.into();
    lists.insert(list.clone());
    list
}

pub(crate) struct MatchedRules {
    pub(crate) checked: std::cell::Cell<u64>,
    pub(crate) cached: Option<CachedNode>,
    pub(crate) dependencies: Box<[crate::dom::Dependency]>,
    pub(crate) rules: Rc<[(usize, [usize; 3])]>,
    pub(crate) pseudos: Rc<[usize]>,
}

pub(crate) type ContainerQuery = Rc<[(Option<String>, bool, MediaQuery)]>;
pub(crate) type MediaStack = Rc<[Rc<MediaBlock>]>;

pub(crate) struct MediaBlock {
    pub(crate) queries: Vec<MediaQuery>,
    pub(crate) matched: std::cell::Cell<bool>,
    pub(crate) fixed: bool,
}

pub(crate) struct Collect<'a> {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) reduced_motion: bool,
    pub(crate) native_theme: u32,
    pub(crate) rules: Vec<Collected<'a>>,
    pub(crate) container: Option<ContainerQuery>,
    pub(crate) media: Option<MediaStack>,
    pub(crate) blocks: Vec<Rc<MediaBlock>>,
    pub(crate) layers: Vec<(String, Vec<u32>)>,
    pub(crate) keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    pub(crate) properties: Registry,
    // The layer of each `@property` (true) and `@keyframes` (false) rule that wins its name.
    pub(crate) layered: HashMap<(bool, String), Option<usize>>,
}

impl Collect<'_> {
    pub(crate) fn media(&self, queries: &[MediaQuery]) -> bool {
        MediaQuery::list(
            queries,
            [self.width, self.height],
            self.reduced_motion,
            self.native_theme,
        )
    }

    // A new layer moves the ranks of others, so this runs once, after every sheet is collected.
    pub(crate) fn rank(&mut self) {
        let mut sorted: Vec<usize> = (0..self.layers.len()).collect();
        sorted.sort_by(|&a, &b| {
            let path = |index: usize| self.layers[index].1.iter().copied().chain([u32::MAX]);
            path(a).cmp(path(b))
        });
        let mut ranks = vec![0; self.layers.len()];
        for (rank, index) in (0..).zip(sorted) {
            ranks[index] = rank;
        }
        for rule in &mut self.rules {
            rule.2.rank = ranks
                .get(rule.2.rank as usize)
                .copied()
                .unwrap_or(UNLAYERED);
        }
    }
}

impl<'a> Collect<'a> {
    pub(crate) fn rule(
        &mut self,
        item: &'a Rule,
        parent: Option<&'a SelectorList>,
        layer: Option<usize>,
    ) {
        match item {
            Rule::Declarations(values) => {
                if let Some(parent) = parent {
                    let rank = layer.map_or(UNLAYERED, |index| index as u32);
                    self.rules.push((
                        parent,
                        values,
                        Layer {
                            origin: AUTHOR,
                            rank,
                        },
                        self.container.clone(),
                        self.media.clone(),
                    ));
                }
            }
            Rule::Theme(_, children) => {
                for rule in children {
                    if matches!(rule, Rule::Keyframes(..)) {
                        self.rule(rule, parent, layer);
                    }
                }
            }
            Rule::Variant(..) | Rule::Utility(..) | Rule::FontFace(_) | Rule::Tailwind(_) => {}
            Rule::Property(name, ..) | Rule::Keyframes(name, _) => {
                let path = |index: Option<usize>| {
                    index
                        .map_or(&[][..], |index| &self.layers[index].1[..])
                        .iter()
                        .copied()
                        .chain([u32::MAX])
                };
                let key = (matches!(item, Rule::Property(..)), name.clone());
                if self
                    .layered
                    .get(&key)
                    .is_some_and(|&old| path(old).gt(path(layer)))
                {
                    return;
                }
                self.layered.insert(key, layer);
                match item {
                    Rule::Property(name, syntax, inherits, initial) => {
                        self.properties.insert(
                            Prop::new(name),
                            (
                                syntax.as_str().into(),
                                *inherits,
                                initial.as_deref().map(Rc::from),
                            ),
                        );
                    }
                    Rule::Keyframes(name, frames) => {
                        self.keyframes.insert(name.clone(), frames.clone());
                    }
                    _ => {}
                }
            }
            Rule::Style(selectors, children) => {
                for rule in children {
                    self.rule(rule, Some(selectors), layer);
                }
            }
            Rule::Layer(names, children) => {
                let names = if names.is_empty() {
                    vec![format!("__anonymous_{}", self.layers.len())]
                } else {
                    names.clone()
                };
                for name in names {
                    let name = match layer {
                        Some(outer) => format!("{}.{name}", self.layers[outer].0),
                        None => name,
                    };
                    let (mut route, mut index) = (Vec::new(), 0);
                    for (end, _) in name.match_indices('.').chain([(name.len(), "")]) {
                        let path = &name[..end];
                        let owner = path.rsplit_once('.').map_or("", |(owner, _)| owner);
                        let found = self.layers.iter().position(|(name, _)| name == path);
                        index = found.unwrap_or(self.layers.len());
                        route = match found {
                            Some(found) => self.layers[found].1.clone(),
                            None => {
                                let siblings = self
                                    .layers
                                    .iter()
                                    .filter(|(name, _)| {
                                        name.rsplit_once('.').map_or("", |(owner, _)| owner)
                                            == owner
                                    })
                                    .count();
                                route.push(siblings as u32);
                                self.layers.push((path.to_owned(), route.clone()));
                                route
                            }
                        };
                    }
                    for rule in children {
                        self.rule(rule, parent, Some(index));
                    }
                }
            }
            Rule::Starting(children) => {
                for rule in children {
                    self.rule(rule, parent, layer);
                }
            }
            Rule::Supports(_, supported, children) => {
                if *supported {
                    for rule in children {
                        self.rule(rule, parent, layer);
                    }
                }
            }
            Rule::Container(name, block, query, children) => {
                let outer = self.container.clone();
                self.container = Some(
                    outer
                        .iter()
                        .flat_map(|conditions| conditions.iter().cloned())
                        .chain([(name.clone(), *block, query.clone())])
                        .collect(),
                );
                for rule in children {
                    self.rule(rule, parent, layer);
                }
                self.container = outer;
            }
            Rule::Media(queries, children) => {
                let matched = self.media(queries);
                let fixed = !children.iter().all(Rule::plain);
                let block = Rc::new(MediaBlock {
                    queries: queries.clone(),
                    matched: matched.into(),
                    fixed,
                });
                self.blocks.push(block.clone());
                if matched || !fixed {
                    let outer = self.media.clone();
                    if !fixed {
                        self.media = Some(
                            outer
                                .iter()
                                .flat_map(|stack| stack.iter().cloned())
                                .chain([block])
                                .collect(),
                        );
                    }
                    for rule in children {
                        self.rule(rule, parent, layer);
                    }
                    self.media = outer;
                }
            }
        }
    }
}
