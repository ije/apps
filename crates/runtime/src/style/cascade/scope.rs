use crate::css::properties::Style;
use crate::css::{
    properties::Prop, properties::Registry, selector::Selector, selector::SelectorList,
    sheet::Declarations, sheet::Declared, sheet::MediaQuery, sheet::Rule, sheet::Stylesheet,
};
use crate::dom::{Document, SelectorCache};
use crate::style::StyleCache;
use crate::style::cascade::TreeWalk;
use crate::style::cascade::container::{ContainerQuery, style_query};
use crate::style::cascade::memo::{CachedNode, CachedStyle, Share};
use crate::style::cascade::rank::{AGENT, AUTHOR, DELETED, Layer, UNLAYERED, USER};
use crate::style::cascade::shadow::{HIGHLIGHT, PART, SLOTTED, TRANSITION};
#[cfg(target_os = "macos")]
use crate::style::ua::GLASS_SHEET;
use crate::style::ua::{FULLSCREEN_SHEET, UA_SHEET};
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::rc::Rc;
pub(crate) type Key = Box<[(u8, String)]>;

thread_local! {
    static UA_KEYFRAMES: Vec<(String, Vec<(f32, Declarations)>)> = UA_SHEET.with(|sheet| {
        (sheet.0.iter())
            .filter_map(|rule| match &**rule {
                Rule::Keyframes(name, frames) => Some((name.clone(), frames.clone())),
                _ => None,
            })
            .collect()
    });
}

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
    pub(crate) states: u32,
    pub(crate) nodes: Vec<Option<MatchedRules>>,
    pub(crate) lists: RuleLists,
    pub(crate) keys: Vec<u128>,
    pub(crate) block: Option<u64>,
    pub(crate) sequence: u64,
    pub(crate) named: bool,
}

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
        matches: &mut crate::dom::SelectorCache,
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
                        if kind == HIGHLIGHT || kind >= TRANSITION {
                            let name = selector.pseudo_argument().unwrap_or_default().into();
                            self.arguments
                                .insert(self.pseudos.len(), (name, Rc::default(), kind));
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
                    matches.note_selector(selector, subject);
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
        self.push(rules, Some(update.keys), matches);
        self.blocks.extend(blocks.iter().cloned());
        Some(self.rematch(start, doc, matches))
    }

    // Forget the matched rules of the nodes that the rules from `start` can match, and mark them dirty. False
    // means that the rules can also reach other nodes, so only a full pass finds them all.
    pub(crate) fn rematch(
        &mut self,
        start: usize,
        doc: &Document,
        matches: &mut crate::dom::SelectorCache,
    ) -> bool {
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
            let lower = crate::css::selector::lowercase;
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
                matches.touch(id);
            }
        }
        !passed
    }
}

// The rules that match a node, by order, with the specificity of the best selector of each.
pub(crate) type Matched = Rc<[(usize, [usize; 3])]>;
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

pub(crate) type MediaStack = Rc<[Rc<MediaBlock>]>;

pub(crate) struct MediaBlock {
    pub(crate) queries: Vec<MediaQuery>,
    pub(crate) matched: std::cell::Cell<bool>,
    pub(crate) fixed: bool,
}

pub(crate) struct Collect<'a> {
    pub(crate) width: f32,
    pub(crate) height: f32,
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
    pub(crate) fn new(width: f32, height: f32, native_theme: u32) -> Self {
        Collect {
            width,
            height,
            native_theme,
            rules: Vec::new(),
            container: None,
            media: None,
            blocks: Vec::new(),
            layers: Vec::new(),
            keyframes: HashMap::new(),
            properties: Registry::default(),
            layered: HashMap::new(),
        }
    }

    pub(crate) fn media(&self, queries: &[MediaQuery]) -> bool {
        MediaQuery::list(
            queries,
            [self.width, self.height],
            self.native_theme & 32 != 0,
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
            Rule::CustomVariant(..)
            | Rule::VariantBlock(..)
            | Rule::Utility(..)
            | Rule::FontFace(_)
            | Rule::Tailwind(_) => {}
            Rule::Property(name, ..) | Rule::Keyframes(name, _) | Rule::PositionTry(name, _) => {
                let path = |index: Option<usize>| {
                    index
                        .map_or(&[][..], |index| &self.layers[index].1[..])
                        .iter()
                        .copied()
                        .chain([u32::MAX])
                };
                let name = match item {
                    Rule::PositionTry(..) => format!("\0{name}"),
                    _ => name.clone(),
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
                    Rule::Property(name, ..) => {
                        if let Some(property) = item.property() {
                            self.properties.insert(Prop::new(name), property);
                        }
                    }
                    Rule::Keyframes(_, frames) => {
                        self.keyframes.insert(name, frames.clone());
                    }
                    // They take their place among the keyframes, under names that no keyframes have.
                    Rule::PositionTry(_, values) => {
                        self.keyframes.insert(name, vec![(0., values.clone())]);
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

impl StyleCache {
    // Chooses the scope of each tree of the walk: an equal one, one patched with the changed rules, or a new one.
    // Returns the ids of the chosen scopes, the `color-scheme` of the page and the Tailwind theme of the document.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn choose_scopes(
        &mut self,
        doc: &Document,
        walk: &TreeWalk,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
        inserted: &[(Rc<Stylesheet>, bool)],
        fullscreen: bool,
        fast: &mut bool,
        [width, height]: [f32; 2],
        native_theme: u32,
    ) -> (Vec<usize>, Option<Rc<str>>, Option<Rc<Style>>) {
        let blank = || Collect::new(width, height, native_theme);
        let user = |sheet: &Rc<Stylesheet>| {
            inserted
                .iter()
                .any(|(inserted, user)| *user && Rc::ptr_eq(inserted, sheet))
        };
        let mut scheme = None;
        let mut theme = None;
        let mut chosen = Vec::with_capacity(walk.trees.len());
        let mut lists: FxHashMap<Vec<usize>, usize> = FxHashMap::default();
        let (mut sheets, mut list) = (Vec::new(), Vec::new());
        for (tree, &(root, _)) in walk.trees.iter().enumerate() {
            sheets.clear();
            #[cfg(target_os = "macos")]
            sheets.push(GLASS_SHEET.with(Clone::clone));
            for &id in &walk.special[tree] {
                let node = &doc.nodes[id];
                if node.tag == "meta"
                    && tree == 0
                    && node
                        .attrs
                        .get("name")
                        .is_some_and(|name| name.eq_ignore_ascii_case("color-scheme"))
                {
                    scheme = node
                        .attrs
                        .get("content")
                        .map(|content| Rc::<str>::from(content.trim()));
                }
                if node.tag != "style"
                    && !(node.tag == "link"
                        && node.stylesheet.is_some()
                        && node.attrs.get("rel").is_some_and(|rel| {
                            rel.split_ascii_whitespace()
                                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                        }))
                {
                    continue;
                }
                let mut ancestor = node.parent;
                while let Some(parent) = ancestor {
                    if matches!(doc.nodes[parent].tag.as_str(), "template" | "noscript") {
                        break;
                    }
                    ancestor = doc.nodes[parent].parent;
                }
                if ancestor.is_some() || node.attrs.contains_key("disabled") {
                    continue;
                }
                if let Some(media) = node
                    .attrs
                    .get("media")
                    .filter(|media| !media.trim().is_empty())
                {
                    let pass = self.pass;
                    let (used, text, query) = self.media.entry(id).or_default();
                    if text != media {
                        (*text, *query) = (
                            media.clone(),
                            Stylesheet::parse(&format!("@media {media}{{}}")),
                        );
                    }
                    *used = pass;
                    if !query.matches_media(width, height, native_theme) {
                        continue;
                    }
                }
                sheets.extend(self.sheet(doc, id, cache));
            }
            if let Some(adopted) = self.adopted.get(&root) {
                sheets.extend(
                    adopted
                        .iter()
                        .filter_map(|id| self.constructed.get(id).cloned()),
                );
            }
            if tree == 0 {
                sheets.extend(inserted.iter().map(|(sheet, _)| sheet.clone()));
            }
            list.clear();
            list.extend(sheets.iter().map(|sheet| Rc::as_ptr(sheet) as usize));
            if let Some(&scope) = lists.get(list.as_slice()) {
                chosen.push(scope);
                continue;
            }
            let mut collected = blank();
            let tailwind = self.tailwind.update(&mut sheets, doc, walk.walk.order());
            if let Some(update) = &tailwind
                && !(update.replaced.is_empty() && update.added.is_empty())
            {
                let mut added = blank();
                for rule in &update.rules {
                    added.rule(rule, None, None);
                }
                let old: Vec<_> = update
                    .replaced
                    .iter()
                    .chain(update.added.iter().map(|(old, _)| old))
                    .collect();
                let count = self.scopes.len();
                let (matches, styled) = (&mut self.matches, &mut self.styled);
                let plain = added.keyframes.is_empty() && added.properties.is_empty();
                self.scopes.retain_mut(|scope| {
                    if lists.values().any(|&id| id == scope.id)
                        || !scope
                            .sheets
                            .iter()
                            .any(|sheet| old.iter().any(|old| Rc::ptr_eq(sheet, old)))
                    {
                        return true;
                    }
                    let local = scope.insert(update, &added.rules, &added.blocks, doc, matches);
                    // The dirty marks stand for the new rules in the last stored pass.
                    match styled.as_mut().filter(|_| plain && local == Some(true)) {
                        Some(styled) => {
                            for old in styled.scopes.iter_mut().filter(|old| old.1 == scope.id) {
                                old.2 = scope.rules.len();
                            }
                        }
                        None => *fast = false,
                    }
                    local.is_some()
                });
                if self.scopes.len() < count {
                    self.matches.forget_selectors();
                }
                if !update.replaced.is_empty() {
                    self.queries.clear();
                }
            }
            if tree == 0 {
                theme = tailwind.as_ref().map(|update| update.root.clone());
            }
            // Collect built-in sheets last so author sheets define layer order. Keep built-in rules first within
            // each layer.
            let builtin = usize::from(cfg!(target_os = "macos"));
            let sources = (builtin..sheets.len()).chain(0..builtin);
            let exact = self.scopes.iter().position(|cached| {
                cached.fullscreen == fullscreen
                    && cached.sheets.len() == sheets.len()
                    && cached
                        .sheets
                        .iter()
                        .zip(&sheets)
                        .all(|(a, b)| Rc::ptr_eq(a, b))
            });
            type Fresh = (Vec<Vec<bool>>, Vec<(Rc<Stylesheet>, usize)>);
            let fresh = |old: &[Rc<Stylesheet>]| -> Option<Fresh> {
                let mut next = 0;
                let mut fresh = Vec::new();
                let mut gone = Vec::new();
                for sheet in &sheets {
                    let mut flags = vec![false; sheet.0.len()];
                    match old.get(next) {
                        Some(previous) if Rc::ptr_eq(previous, sheet) => next += 1,
                        Some(previous)
                            if self.derived.get(&Rc::as_ptr(sheet)).is_some_and(
                                |(weak, base, ..)| {
                                    weak.as_ptr() == Rc::as_ptr(sheet) && Rc::ptr_eq(base, previous)
                                },
                            ) =>
                        {
                            next += 1;
                            let (_, base, inserted, removed) = &self.derived[&Rc::as_ptr(sheet)];
                            for &index in inserted {
                                *flags.get_mut(index)? = true;
                            }
                            gone.extend(removed.iter().map(|&index| (base.clone(), index)));
                        }
                        // A sheet in the place of one that left, like a `<style>` with new text, loses all old rules.
                        Some(previous)
                            if !sheets.iter().any(|sheet| Rc::ptr_eq(sheet, previous))
                                && !old.iter().any(|previous| Rc::ptr_eq(previous, sheet)) =>
                        {
                            next += 1;
                            flags.fill(true);
                            gone.extend(
                                (0..previous.0.len()).map(|index| (previous.clone(), index)),
                            );
                        }
                        _ if old.iter().any(|previous| Rc::ptr_eq(previous, sheet)) => return None,
                        _ => flags.fill(true),
                    }
                    fresh.push(flags);
                }
                (next == old.len()).then_some((fresh, gone))
            };
            let incremental = exact
                .is_none()
                .then(|| {
                    self.scopes.iter().enumerate().find_map(|(index, cached)| {
                        (lists.values().all(|&id| id != cached.id)
                            && cached.fullscreen == fullscreen)
                            .then(|| fresh(&cached.sheets))
                            .flatten()
                            .map(|fresh| (index, fresh))
                    })
                })
                .flatten();
            let scope = match (exact, incremental) {
                (Some(scope), _) => scope,
                (None, Some((index, (fresh, gone)))) => {
                    let mut new = Vec::new();
                    for index in sources.clone() {
                        let (sheet, flags) = (&sheets[index], &fresh[index]);
                        let first = collected.rules.len();
                        for (rule, &flag) in sheet.0.iter().zip(flags) {
                            let start = collected.rules.len();
                            collected.rule(rule, None, None);
                            new.extend(std::iter::repeat_n(flag, collected.rules.len() - start));
                        }
                        if builtin != 0 && index == 0 {
                            let count = collected.rules.len() - first;
                            collected.rules.rotate_right(count);
                            new.rotate_right(count);
                        }
                        if user(sheet) {
                            for rule in &mut collected.rules[first..] {
                                rule.2.origin = USER;
                            }
                        }
                    }
                    collected.rank();
                    let mut removed = blank();
                    for (sheet, index) in &gone {
                        if let Some(rule) = sheet.0.get(*index) {
                            removed.rule(rule, None, None);
                        }
                    }
                    let cached = &mut self.scopes[index];
                    let removed: rustc_hash::FxHashSet<_> = removed
                        .rules
                        .iter()
                        .map(|rule| Rc::as_ptr(rule.0))
                        .collect();
                    let mut deleted: Vec<bool> = (cached.rules.iter())
                        .map(|rule| removed.contains(&Rc::as_ptr(&rule.0)))
                        .collect();
                    // Preserve deleted rule positions with DELETED origin and no declarations or keys.
                    let mut author: Vec<usize> = (0..cached.rules.len())
                        .filter(|&order| {
                            !matches!(cached.rules[order].2.origin, AGENT | DELETED)
                                && !deleted[order]
                        })
                        .collect();
                    let dead = (cached.rules.iter().zip(&deleted))
                        .filter(|(rule, deleted)| **deleted || rule.2.origin == DELETED)
                        .count();
                    author.sort_by_key(|&order| cached.keys[order]);
                    let old = new.iter().filter(|flag| !**flag).count();
                    let same = old == author.len()
                        && collected.layers == cached.layers
                        && collected
                            .rules
                            .iter()
                            .zip(&new)
                            .filter(|(_, flag)| !**flag)
                            .zip(&author)
                            .all(|(((selectors, ..), _), &order)| {
                                Rc::ptr_eq(selectors, &cached.rules[order].0)
                            });
                    // A scope with more deleted rules than live ones is built again, so it does not grow.
                    let mut room = same && dead * 2 <= cached.rules.len() + new.len() - old;
                    let mut keys = Vec::new();
                    let mut inserted = Vec::new();
                    let mut pending = Vec::new();
                    let block = cached.block;
                    let generated = |key: u128| {
                        block.is_some_and(|block| (key >> 64) as u64 == block && key as u64 != 0)
                    };
                    // Another Tailwind configuration makes all its utilities again, with keys of their own.
                    room &= !(cached.keys.iter().zip(&deleted))
                        .any(|(key, deleted)| *deleted && generated(*key));
                    if same {
                        let mut olds = author.iter().copied();
                        let mut previous = cached
                            .keys
                            .iter()
                            .zip(&cached.rules)
                            .filter(|(_, rule)| rule.2.origin == AGENT)
                            .map(|(key, _)| *key)
                            .max()
                            .unwrap_or(0);
                        for (rule, &flag) in collected.rules.iter().zip(&new) {
                            if flag {
                                pending.push(rule.clone());
                                continue;
                            }
                            let next = cached.keys[olds.next().unwrap_or_default()];
                            if !pending.is_empty() {
                                let step =
                                    ((next - previous) / (pending.len() as u128 + 1)).min(1 << 20);
                                room &= step > 0 && !generated(previous) && !generated(next);
                                for (index, rule) in pending.drain(..).enumerate() {
                                    keys.push(previous + step * (index as u128 + 1));
                                    inserted.push(rule);
                                }
                            }
                            previous = next;
                        }
                    }
                    if room {
                        let start = cached.rules.len();
                        cached.push(inserted, None, &mut self.matches);
                        for (order, key) in (start..).zip(keys) {
                            cached.keys[order] = key;
                        }
                        cached.push(pending, None, &mut self.matches);
                        let local = cached.rematch(start, doc, &mut self.matches);
                        let Collect {
                            mut keyframes,
                            properties,
                            blocks,
                            ..
                        } = std::mem::replace(&mut collected, blank());
                        // The keyframes of the built-in sheet stay under those of the author.
                        UA_KEYFRAMES.with(|built| {
                            for (name, frames) in built {
                                keyframes
                                    .entry(name.clone())
                                    .or_insert_with(|| frames.clone());
                            }
                        });
                        let plain = local
                            && cached.keyframes == keyframes
                            && cached.properties == properties;
                        // The dirty marks stand for the new rules in the last stored pass.
                        match self.styled.as_mut().filter(|_| plain) {
                            Some(styled) => {
                                for old in styled.scopes.iter_mut().filter(|old| old.1 == cached.id)
                                {
                                    old.2 = cached.rules.len();
                                }
                            }
                            None => *fast = false,
                        }
                        if deleted.contains(&true) {
                            deleted.resize(cached.rules.len(), false);
                            for order in (0..deleted.len()).filter(|&order| deleted[order]) {
                                cached.rules[order].1 = Rc::new([]);
                                cached.rules[order].2.origin = DELETED;
                                cached.elements[order].clear();
                            }
                            let gone = |entry: &usize| {
                                if *entry >= PSEUDO {
                                    deleted[cached.pseudos[entry & !PSEUDO].2]
                                } else {
                                    deleted[entry >> SELECTOR]
                                }
                            };
                            for bucket in &mut cached.buckets {
                                bucket.retain(|_, list| {
                                    list.retain(|entry| !gone(entry));
                                    !list.is_empty()
                                });
                            }
                            cached.universal.retain(|entry| !gone(entry));
                            let stale = |rules: &[(usize, [usize; 3])], pseudos: &[usize]| {
                                rules.iter().any(|&(order, _)| deleted[order])
                                    || pseudos
                                        .iter()
                                        .any(|&index| deleted[cached.pseudos[index].2])
                            };
                            // A fast pass styles these nodes again by their dirty marks.
                            for (id, matched) in cached.nodes.iter_mut().enumerate() {
                                if matched
                                    .as_ref()
                                    .is_some_and(|matched| stale(&matched.rules, &matched.pseudos))
                                {
                                    *matched = None;
                                    self.matches.mark(id, false);
                                }
                            }
                            self.values.retain(|key, _| {
                                key.scope != cached.id || !stale(&key.rules.0, &key.pseudos)
                            });
                            // The interaction states come from the rules that are left.
                            let mut lists: Vec<_> = (cached.rules.iter())
                                .filter(|rule| rule.2.origin != DELETED)
                                .map(|rule| rule.0.clone())
                                .collect();
                            let (mut seen, mut parents, mut subject) =
                                (std::collections::HashSet::new(), Vec::new(), true);
                            cached.states = 0;
                            while let Some(list) = lists.pop().or_else(|| {
                                subject = false;
                                parents.pop()
                            }) {
                                if seen.insert((Rc::as_ptr(&list), subject)) {
                                    for selector in list.iter() {
                                        cached.states |= selector.states(&mut parents);
                                    }
                                }
                            }
                        }
                        if cached.keyframes != keyframes {
                            self.rules += 1;
                            let animated = |value: &CachedStyle| {
                                value.style.contains(Prop::ANIMATION)
                                    || value.style.contains(Prop::ANIMATION_NAME)
                            };
                            for node in cached.nodes.iter_mut().flatten() {
                                if node
                                    .cached
                                    .as_ref()
                                    .is_some_and(|node| animated(&node.value))
                                {
                                    node.cached = None;
                                }
                            }
                            self.values.retain(|key, values| {
                                key.scope != cached.id || {
                                    values.retain(|value| !animated(value));
                                    !values.is_empty()
                                }
                            });
                        }
                        cached.sheets = sheets.clone();
                        let mut seen = std::collections::HashSet::new();
                        cached.blocks = cached
                            .medias
                            .iter()
                            .flatten()
                            .flat_map(|stack| stack.iter())
                            .chain(blocks.iter().filter(|block| block.fixed))
                            .filter(|block| seen.insert(Rc::as_ptr(block)))
                            .cloned()
                            .collect();
                        cached.keyframes = keyframes;
                        cached.properties = properties;
                        index
                    } else {
                        collected.rules.clear();
                        collected.layers.clear();
                        collected.keyframes.clear();
                        collected.properties.clear();
                        collected.layered.clear();
                        collected.blocks.clear();
                        usize::MAX
                    }
                }
                (None, None) => usize::MAX,
            };
            let scope = if scope != usize::MAX {
                scope
            } else {
                for index in sources {
                    let sheet = &sheets[index];
                    let first = collected.rules.len();
                    for rule in &sheet.0 {
                        collected.rule(rule, None, None);
                    }
                    if builtin != 0 && index == 0 {
                        let count = collected.rules.len() - first;
                        collected.rules.rotate_right(count);
                    }
                    if user(sheet) {
                        for rule in &mut collected.rules[first..] {
                            rule.2.origin = USER;
                        }
                    }
                }
                let author = std::mem::take(&mut collected.rules);
                let ua = if fullscreen {
                    &FULLSCREEN_SHEET
                } else {
                    &UA_SHEET
                }
                .with(Clone::clone);
                for rule in &ua.0 {
                    collected.rule(rule, None, None);
                }
                for rule in &mut collected.rules {
                    rule.2.origin = AGENT;
                }
                collected.rules.extend(author);
                collected.rank();
                let Collect {
                    rules,
                    keyframes,
                    properties,
                    layers,
                    blocks,
                    ..
                } = collected;
                self.next += 1;
                self.rules += 1;
                let mut scope = StyleScope {
                    id: self.next,
                    fullscreen,
                    sheets: sheets.clone(),
                    layers,
                    blocks,
                    keyframes,
                    properties,
                    ..Default::default()
                };
                scope.push(
                    rules,
                    tailwind.as_ref().map(|update| update.keys),
                    &mut self.matches,
                );
                self.scopes.push(scope);
                self.scopes.len() - 1
            };
            let scoped = &mut self.scopes[scope];
            scoped.used = self.pass;
            if scoped.nodes.len() < doc.nodes.len() {
                scoped.nodes.resize_with(doc.nodes.len(), || None);
            }
            lists.insert(list.clone(), scoped.id);
            chosen.push(scoped.id);
        }
        (chosen, scheme, theme)
    }
}

impl StyleCache {
    pub(crate) fn font_faces(&self) -> Vec<crate::css::sheet::FontFace> {
        let mut seen = std::collections::HashSet::new();
        self.scopes
            .iter()
            .flat_map(|scope| &scope.sheets)
            .filter(|sheet| seen.insert(Rc::as_ptr(sheet)))
            .flat_map(|sheet| sheet.font_faces())
            .collect()
    }
}

impl StyleCache {
    pub(crate) fn set_containers(
        &mut self,
        sizes: &FxHashMap<usize, [f32; 2]>,
        doc: &Document,
    ) -> bool {
        self.units.clear();
        if *sizes == self.containers {
            return false;
        }
        // Styles with container units are not shared, so only the nodes in a resized container need a new style.
        let mut flipped: rustc_hash::FxHashSet<usize> = self
            .sized
            .iter()
            .copied()
            .filter(|id| sizes.get(id) != self.containers.get(id))
            .collect();
        self.units.extend(&flipped);
        for (&(scope, order, index, container), &(inline, matched, lengths)) in &self.queries {
            let query = self
                .scopes
                .iter()
                .find(|cached| cached.id == scope)
                .and_then(|scope| scope.containers.get(order)?.as_ref()?.get(index));
            if query.is_none_or(|(_, _, query)| {
                sizes.get(&container).is_some_and(|size| {
                    query.matches(
                        size[0],
                        if inline { f32::NAN } else { size[1] },
                        false,
                        0,
                        lengths,
                    ) == Some(true)
                }) != matched
            }) {
                flipped.insert(container);
            }
        }
        self.containers = sizes.clone();
        if flipped.is_empty() {
            return false;
        }
        self.queries
            .retain(|&(_, _, _, container), _| !flipped.contains(&container));
        let mut pending: Vec<_> = flipped.iter().map(|&id| (id, true)).collect();
        while let Some((id, root)) = pending.pop() {
            // A flipped container inside another one is walked once, as its own root.
            let Some(node) = doc.nodes.get(id).filter(|_| root || !flipped.contains(&id)) else {
                continue;
            };
            pending.extend(
                self.shadows
                    .composed
                    .get(&id)
                    .unwrap_or(&node.children)
                    .iter()
                    .map(|&child| (child, false)),
            );
            for scope in &mut self.scopes {
                if let Some(Some(node)) = scope.nodes.get_mut(id) {
                    node.cached = None;
                }
            }
            self.matches.mark(id, false);
        }
        true
    }
}

impl StyleScope {
    // The rules and pseudo-element rules that match a node: the last match while what it read holds, the match of
    // a like sibling, or a match of the candidates that the keys of the node find in the buckets.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn match_node(
        &mut self,
        doc: &Document,
        id: usize,
        matches: &mut SelectorCache,
        group: Option<(usize, [u32; 7])>,
        shares: &mut FxHashMap<(usize, [u32; 7]), Share>,
        candidates: &mut Vec<usize>,
        no_pseudos: &Rc<[usize]>,
    ) -> (Matched, Rc<[usize]>) {
        let node = &doc.nodes[id];
        if let Some(cached) = self.nodes[id].as_ref()
            && matches.valid_since(&cached.dependencies, &cached.checked)
        {
            (cached.rules.clone(), cached.pseudos.clone())
        } else if let Some(kind) = node.tag.strip_prefix("::") {
            // A view transition pseudo-element takes the rules that its kind matched on the root.
            let root = std::iter::successors(Some(id), |&at| doc.nodes[at].parent)
                .find(|&at| doc.nodes[at].tag == "html")
                .unwrap_or(id);
            let kind = [
                "view-transition",
                "view-transition-group",
                "view-transition-image-pair",
            ]
            .iter()
            .chain(&["view-transition-old", "view-transition-new"])
            .position(|name| *name == kind)
            .map_or(usize::MAX, |at| TRANSITION + at);
            let (name, classes) = (node.attrs.get("name"), node.attrs.get("class"));
            let found = |argument: &str| {
                let mut parts = argument.split('.');
                parts.next().is_some_and(|first| {
                    matches!(first, "*" | "") || Some(first) == name.map(|n| n.as_str())
                }) && parts
                    .all(|class| classes.is_some_and(|all| all.split(' ').any(|own| own == class)))
            };
            let mut matched: Vec<(usize, [usize; 3])> = (self.nodes[root].as_ref())
                .into_iter()
                .flat_map(|root| root.pseudos.iter())
                .filter_map(|&index| {
                    let (own, selector, order, _) = &self.pseudos[index];
                    (*own == kind
                        && self
                            .arguments
                            .get(&index)
                            .is_none_or(|(argument, ..)| found(argument)))
                    .then_some((*order, selector.specificity))
                })
                .collect();
            matched.sort_unstable();
            matches.begin();
            matches.depend(id, 0);
            for kind in 0..9 {
                matches.depend(root, kind);
            }
            let rules = intern(&mut self.lists, &matched);
            self.nodes[id] = Some(MatchedRules {
                checked: std::cell::Cell::new(matches.epoch),
                cached: None,
                dependencies: matches.recorded().into(),
                rules: rules.clone(),
                pseudos: no_pseudos.clone(),
            });
            (rules, no_pseudos.clone())
        } else if node.tag.starts_with('#') {
            let none = intern(&mut self.lists, &[]);
            self.nodes[id] = Some(MatchedRules {
                checked: std::cell::Cell::new(matches.epoch),
                cached: None,
                dependencies: Box::default(),
                rules: none.clone(),
                pseudos: no_pseudos.clone(),
            });
            (none, no_pseudos.clone())
        } else if let Some(share) = group.and_then(|group| shares.get(&group))
            && share
                .style
                .is_none_or(|style| style == matches.signature(id)[6])
            && matches.share(doc, share.from, id, &share.dependencies)
        {
            matches.register(id);
            self.nodes[id] = Some(MatchedRules {
                checked: std::cell::Cell::new(matches.epoch),
                cached: None,
                dependencies: matches.recorded().into(),
                rules: share.rules.clone(),
                pseudos: share.pseudos.clone(),
            });
            (share.rules.clone(), share.pseudos.clone())
        } else {
            matches.begin();
            // Candidates come from the node's classes, so its matches always read them.
            matches.depend(id, 0);
            matches.depend(id, 7);
            candidates.clear();
            candidates.extend(&self.universal);
            let classes = node.attrs.get("class").into_iter();
            // Keys are lowercase. Only SVG names have uppercase letters, so HTML names need no copy.
            let lower = crate::css::selector::lowercase;
            for (kind, name) in std::iter::once((2, lower(&node.tag)))
                .chain(node.attrs.get("id").map(|id| (0, id.as_str().into())))
                .chain(
                    classes.flat_map(|c| c.split_ascii_whitespace().map(|class| (1, class.into()))),
                )
                .chain(node.attrs.keys().map(|name| (3, lower(name))))
            {
                if let Some(list) = self.buckets[kind].get(&*name) {
                    candidates.extend(list);
                }
            }
            if !self.buckets[4].is_empty() {
                let mut pair = String::new();
                for (name, value) in node.attrs.iter() {
                    pair.clear();
                    pair.push_str(&lower(name));
                    pair.push('=');
                    pair.push_str(value);
                    if let Some(list) = self.buckets[4].get(pair.as_str()) {
                        candidates.extend(list);
                    }
                }
            }
            let fresh = self.nodes[id].is_none();
            let ancestors = matches.ancestors(doc, id);
            let mut read = 0u16;
            let mut test = |selector: &Selector, matches: &mut SelectorCache| {
                if let Some(ancestry) = selector.ancestry()
                    && ancestry
                        .0
                        .iter()
                        .zip(&ancestors)
                        .any(|(need, have)| need & !have != 0)
                {
                    matches.reject(doc, id, ancestry, ancestry.2 & !read);
                    read |= ancestry.2;
                    return false;
                }
                selector.always()
                    || !selector.host_only()
                        && if fresh {
                            matches.matches_fresh(selector, doc, id)
                        } else {
                            matches.matches(selector, doc, id, None)
                        }
            };
            let (mut matched, mut matched_pseudos) = (Vec::new(), Vec::new());
            for &entry in candidates.iter() {
                if entry >= PSEUDO {
                    let index = entry & !PSEUDO;
                    // View transition pseudo-elements come only from the root.
                    if (self.pseudos[index].0 < TRANSITION
                        || node.tag == "html" && node.parent == Some(0))
                        && test(&self.pseudos[index].1, matches)
                    {
                        matched_pseudos.push(index);
                    }
                    continue;
                }
                let (order, index) = (entry >> SELECTOR, entry & ((1 << SELECTOR) - 1));
                let selector = &self.rules[order].0[index];
                if test(selector, matches) {
                    matched.push((order, selector.specificity));
                }
            }
            matched.sort_unstable();
            matched.dedup_by(|later, earlier| {
                earlier.0 == later.0 && {
                    earlier.1 = earlier.1.max(later.1);
                    true
                }
            });
            matched_pseudos.sort_unstable();
            matched_pseudos.dedup();
            // Table parts take `cellpadding`, `border` and `rules` from their table.
            if matches!(
                node.tag.as_str(),
                "td" | "th" | "thead" | "tbody" | "tfoot" | "colgroup"
            ) {
                let mut at = node.parent;
                for _ in 0..3 {
                    let Some(parent) = at else { break };
                    matches.depend(parent, 0);
                    matches.depend(parent, 1);
                    at = doc.nodes[parent].parent;
                }
            }
            let style = matches.reads(id, 6);
            matches.depend(id, 6);
            matches.finish_repeated(id);
            let shared = intern(&mut self.lists, &matched);
            let matched_pseudos = if matched_pseudos.is_empty() {
                no_pseudos.clone()
            } else {
                Rc::from(matched_pseudos)
            };
            if let Some(group) = group
                && matches.shareable(doc, id)
            {
                shares.insert(
                    group,
                    Share {
                        from: id,
                        rules: shared.clone(),
                        pseudos: matched_pseudos.clone(),
                        dependencies: matches.recorded().into(),
                        style: style.then(|| matches.signature(id)[6]),
                    },
                );
            }
            self.nodes[id] = Some(MatchedRules {
                checked: std::cell::Cell::new(matches.epoch),
                cached: None,
                dependencies: matches.recorded().into(),
                rules: shared.clone(),
                pseudos: matched_pseudos.clone(),
            });
            (shared, matched_pseudos)
        }
    }
}
