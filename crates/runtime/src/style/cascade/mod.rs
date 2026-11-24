use crate::css::{
    Declarations, FontUnits, Lengths, Prop, Registry, Selector, Style, Stylesheet, Vars, color,
    number,
};
use crate::dom::{Document, SelectorCache};
use crate::style::animation::Animation;
use crate::style::cascade::container::{QueryCache, query_containers};
use crate::style::cascade::memo::{
    CachedNode, CachedStyle, Inheritable, Inputs, Share, StyleKey, StyleRef,
};
use crate::style::cascade::rank::{AGENT, ANIMATION, AUTHOR, CascadeRank, HINT, physical, put};
use crate::style::cascade::scope::{
    Collect, ContainerQuery, MatchedRules, MediaBlock, MediaStack, PSEUDO, SELECTOR, StyleScope,
    UNLAYERED, intern,
};
use crate::style::cascade::shadow::{ELEMENT, PART, SLOTTED, STARTING, ShadowRule, exported_parts};
use crate::style::cascade::sheets::{Cssom, Derived, Sheet, derive};
use crate::style::cascade::table::CollapsedTable;
use crate::style::cascade::used::{blockify, border_spacing, pixels};
use crate::style::cascade::vars::{references, var};
use crate::style::computed::{HEADER, INVISIBLE};
use crate::style::fonts::font_names;
use crate::style::ua::{presentation, svg_transform};
use crate::style::{
    AGENT_DISPLAY, CAPTION, CELL, COUNTED, ComputedStyles, DISPLAY_CONTENTS, DISPLAY_NONE, FOOTER,
    ROW, SELECT_ALL, SELECT_NONE, SELECT_TEXT, Shadowed, TABLE_DISPLAYS, empty, font, table_box,
};
use bbtree::unsync::BBTreeMap;
use counters::{list_item, listed, number_lists};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use table::anonymous_tables;
mod container;
mod counters;
mod memo;
pub(super) mod rank;
pub(super) mod scope;
mod shadow;
mod sheets;
mod table;
mod used;
mod vars;

pub(crate) use container::Container;
pub(crate) use shadow::Passed;
pub(crate) use used::{dark, drawable_canvas, resolve_keywords};
pub(crate) use vars::resolve_style;

#[derive(Default)]
pub(crate) struct StyleCache {
    pub(crate) environment: Environment,
    pub(crate) scopes: Vec<StyleScope>,
    pub(crate) next: usize,
    pub(crate) pass: u64,
    pub(crate) states: u16,
    pub(crate) sheets: HashMap<usize, Sheet>,
    pub(crate) cssom: HashMap<usize, Cssom>,
    pub(crate) constructed: FxHashMap<u32, Rc<Stylesheet>>,
    pub(crate) derived: Derived,
    pub(crate) adopted: FxHashMap<usize, Vec<u32>>,
    pub(crate) values: rustc_hash::FxHashMap<StyleKey, Vec<Rc<CachedStyle>>>,
    // Values keys hold tag names by these numbers, which are never freed: a page uses few tag names.
    pub(crate) tags: FxHashMap<String, u32>,
    pub(crate) heritage: FxHashMap<*const Style, (Rc<Style>, Rc<Style>)>,
    pub(crate) inherited: rustc_hash::FxHashSet<Rc<Style>>,
    pub(crate) containers: FxHashMap<usize, [f32; 2]>,
    pub(crate) sized: std::collections::HashSet<usize>,
    pub(crate) queries: QueryCache,
    // Resized containers with nodes that use container units.
    pub(crate) units: Vec<usize>,
    pub(crate) changed: Vec<u64>,
    pub(crate) stamp: u64,
    pub(crate) tables: FxHashMap<usize, CollapsedTable>,
    // Parents of table parts that are outside a table, so they can hold an anonymous table.
    pub(crate) orphans: Vec<usize>,
    pub(crate) tailwind: crate::style::tailwind::Compiler,
    // Retain source lists used as pointer keys to prevent address reuse.
    pub(crate) inheritable: FxHashMap<*const Vec<(Prop, Rc<str>)>, Inheritable>,
    pub(crate) scripted: Registry,
    pub(crate) walk: Option<Rc<TreeWalk>>,
    pub(crate) shadows: Rc<Shadows>,
    pub(crate) matches: SelectorCache,
    // An incremental pass starts from the last stored output, so `full` and `styled` must always describe that
    // output.
    pub(crate) full: bool,
    pub(crate) styled: Option<Styled>,
    pub(crate) unshared: bool,
    pub(crate) named: bool,
    #[cfg(debug_assertions)]
    pub(crate) incremental: u64,
}

// An incremental pass may start only from a stored pass that styled with the same environment.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Environment {
    pub(crate) viewport: [f32; 2],
    pub(crate) metrics: Option<f32>,
    pub(crate) reduced_motion: bool,
    pub(crate) native_theme: u32,
    pub(crate) registry: Rc<Registry>,
    pub(crate) scheme: Option<Rc<str>>,
}

// `scopes` must stay sorted by root, and a tree that the stored pass styled must use the same scope again.
pub(crate) struct Styled {
    pub(crate) nodes: Rc<Vec<(u32, u32)>>,
    pub(crate) serial: u64,
    pub(crate) tree: Rc<Vec<u32>>,
    pub(crate) trees: Rc<Vec<(usize, usize)>>,
    pub(crate) environment: Environment,
    pub(crate) scopes: Vec<(usize, usize, usize)>,
}

impl StyleCache {
    pub(crate) fn walk(&mut self, doc: &Document, shadows: &Rc<Shadows>) -> Rc<TreeWalk> {
        let walk = self.walk.take().and_then(|walk| Rc::try_unwrap(walk).ok());
        let mut walk = walk.unwrap_or_default();
        let serial = walk.walk.serial;
        // This skip relies on a new shadow map whenever the tree or its shadow roots change.
        if !(walk.built.0 == doc.nodes.tree() && Rc::ptr_eq(&walk.built.1, shadows))
            && walk.walk.update(doc, &shadows.composed, None)
        {
            let special = |id: usize| {
                matches!(
                    doc.nodes[id].tag.as_str(),
                    "meta" | "style" | "link" | "html" | "table"
                )
            };
            // A patched walk keeps the lists when no node in them, and no node for them, moved or changed.
            // `tree` then stays as it is: without shadow trees, every node is in the first tree.
            let patched = shadows.hosts.is_empty()
                && walk.trees.len() == 1
                && walk.walk.since(serial).is_some_and(|touched| {
                    let mut listed = walk.special[0].iter().chain(&walk.tables);
                    touched.len() <= 256
                        && !listed.any(|&id| touched.contains(&(id as u32)))
                        && touched
                            .iter()
                            .all(|&id| !(walk.walk.contains(id as usize) && special(id as usize)))
                });
            if !patched || cfg!(debug_assertions) {
                let mut tree = vec![u32::MAX; doc.nodes.len()];
                let mut trees = vec![(0, usize::MAX)];
                let mut lists = vec![Vec::new()];
                let mut tables = Vec::new();
                for &(id, _) in walk.walk.nodes.iter() {
                    let id = id as usize;
                    let node = &doc.nodes[id];
                    let index = match node.parent {
                        Some(parent) => tree[parent].min(trees.len() as u32 - 1) as usize,
                        None => match shadows.hosts.get(&id) {
                            Some(&host) => {
                                trees.push((id, host));
                                lists.push(Vec::new());
                                trees.len() - 1
                            }
                            None => 0,
                        },
                    };
                    tree[id] = index as u32;
                    match node.tag.as_str() {
                        "table" => tables.push(id),
                        _ if special(id) => lists[index].push(id),
                        _ => {}
                    }
                }
                assert!(
                    !patched
                        || (&trees, &lists, &tables) == (&*walk.trees, &walk.special, &walk.tables),
                    "patched style walk lists differ"
                );
                if !patched {
                    walk.tree = Rc::new(tree);
                    walk.trees = Rc::new(trees);
                    (walk.special, walk.tables) = (lists, tables);
                }
            }
        }
        walk.built = (doc.nodes.tree(), shadows.clone());
        walk.used.set(self.pass);
        let walk = Rc::new(walk);
        self.walk = Some(walk.clone());
        walk
    }

    pub(crate) fn document_walk(&self) -> Option<&crate::dom::Walk> {
        self.walk.as_ref().map(|walk| &walk.walk)
    }

    pub(crate) fn forget(&mut self, gone: &[usize]) {
        for &id in gone {
            for scope in &mut self.scopes {
                if let Some(node) = scope.nodes.get_mut(id) {
                    *node = None;
                }
            }
            self.matches.forget(id);
            self.sized.remove(&id);
        }
    }

    pub(crate) fn trim(&mut self, len: usize) {
        self.values = Default::default();
        self.walk = None;
        for scope in &mut self.scopes {
            scope.nodes.truncate(len);
            scope.nodes.shrink_to_fit();
            scope.lists = Default::default();
        }
        self.matches.trim(len);
        self.sized.retain(|&id| id < len);
    }

    pub(crate) fn uncache(&mut self) {
        self.values.clear();
        for scope in &mut self.scopes {
            for node in scope.nodes.iter_mut().flatten() {
                node.cached = None;
            }
        }
    }

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

    pub(crate) fn font_faces(&self) -> Vec<(String, String, Option<String>)> {
        let mut seen = std::collections::HashSet::new();
        self.scopes
            .iter()
            .flat_map(|scope| &scope.sheets)
            .filter(|sheet| seen.insert(Rc::as_ptr(sheet)))
            .flat_map(|sheet| sheet.font_faces())
            .collect()
    }

    pub(crate) fn sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    ) -> Option<Rc<Stylesheet>> {
        let node = &doc.nodes[id];
        let pass = self.pass;
        let bundled = node.stylesheet.as_ref().or(match node.children[..] {
            [text] => doc.nodes[text].stylesheet.as_ref(),
            _ => None,
        });
        let same = |text: &str| {
            let mut rest = text;
            let mut pending = vec![id];
            while let Some(child) = pending.pop() {
                let node = &doc.nodes[child];
                if child != id && node.tag == "#comment" {
                    continue;
                }
                match rest.strip_prefix(node.text.as_str()) {
                    Some(tail) if child == id || node.stylesheet.is_none() => rest = tail,
                    _ => return false,
                }
                pending.extend(node.children.iter().rev());
            }
            rest.is_empty()
        };
        if let Some(edited) = self.cssom.get_mut(&id).filter(|edited| {
            edited.bundled == bundled.is_some() && (bundled.is_some() || same(&edited.text))
        }) {
            edited.used = pass;
            return (!edited.disabled).then(|| edited.sheet.clone());
        }
        self.cssom.remove(&id);
        if let Some(sheet) = bundled {
            return Some(sheet.clone());
        }
        let stamp = node
            .children
            .iter()
            .try_fold(doc.nodes.revision(id), |stamp, &child| {
                doc.nodes[child]
                    .children
                    .is_empty()
                    .then(|| stamp.max(doc.nodes.revision(child)))
            });
        if let Some((used, old, text, sheet)) = self.sheets.get_mut(&id)
            && (stamp.is_some() && *old == stamp || same(text))
        {
            *used = pass;
            *old = stamp;
            return Some(sheet.clone());
        }
        let text = doc.text(id);
        let (text, sheet) = match cache.get_key_value(text.as_str()) {
            Some((text, sheet)) => (text.clone(), sheet.clone()),
            None => {
                let previous = self.sheets.get(&id).filter(|(_, _, old, _)| {
                    let (mut depth, mut quote, mut comment, mut escaped) = (0i32, 0u8, None, false);
                    let bytes = old.as_bytes();
                    for (index, &byte) in bytes.iter().enumerate() {
                        match byte {
                            _ if escaped => escaped = false,
                            b'\\' => escaped = true,
                            b'/' if comment.is_some_and(|open| {
                                index > open + 1 && bytes[index - 1] == b'*'
                            }) =>
                            {
                                comment = None
                            }
                            _ if comment.is_some() => {}
                            b'*' if quote == 0 && index > 0 && bytes[index - 1] == b'/' => {
                                comment = Some(index)
                            }
                            b'"' | b'\'' if quote == 0 => quote = byte,
                            _ if quote == byte => quote = 0,
                            b'{' if quote == 0 => depth += 1,
                            b'}' if quote == 0 => depth -= 1,
                            _ => {}
                        }
                    }
                    text.starts_with(&**old)
                        && depth == 0
                        && quote == 0
                        && comment.is_none()
                        && !escaped
                        && old.trim_end().ends_with(['}', ';'])
                });
                let sheet = match previous {
                    Some((_, _, old, base)) => {
                        let mut rules = base.0.clone();
                        let start = rules.len();
                        rules.extend(Stylesheet::parse(&text[old.len()..]).0);
                        let sheet = Rc::new(Stylesheet(rules));
                        self.derived.insert(
                            Rc::as_ptr(&sheet),
                            (
                                Rc::downgrade(&sheet),
                                base.clone(),
                                (start..sheet.0.len()).collect(),
                                Vec::new(),
                            ),
                        );
                        sheet
                    }
                    None => Rc::new(Stylesheet::parse(&text)),
                };
                let text: Rc<str> = text.into();
                cache.insert(text.clone(), sheet.clone());
                (text, sheet)
            }
        };
        self.sheets.insert(id, (pass, stamp, text, sheet.clone()));
        Some(sheet)
    }

    pub(crate) fn edit_constructed(
        &mut self,
        id: u32,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        match self.constructed.get_mut(&id) {
            Some(sheet) => derive(&mut self.derived, sheet, top, change),
            None => Err("InvalidStateError"),
        }
    }

    pub(crate) fn registry(&self) -> Rc<Registry> {
        self.environment.registry.clone()
    }

    pub(crate) fn edit_sheet(
        &mut self,
        doc: &Document,
        id: usize,
        cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
        top: Option<(usize, bool)>,
        change: impl FnOnce(&mut Stylesheet, &mut bool) -> Result<serde_json::Value, &'static str>,
    ) -> Result<serde_json::Value, &'static str> {
        let current = self.sheet(doc, id, cache);
        let (mut sheet, mut disabled) = self.cssom.remove(&id).map_or_else(
            || (current.unwrap_or_default(), false),
            |edited| (edited.sheet, edited.disabled),
        );
        let result = derive(&mut self.derived, &mut sheet, top, |sheet| {
            change(sheet, &mut disabled)
        });
        let mut text = String::new();
        let mut pending = vec![id];
        while let Some(child) = pending.pop() {
            let node = &doc.nodes[child];
            if child == id || node.tag != "#comment" {
                text.push_str(&node.text);
                pending.extend(node.children.iter().rev());
            }
        }
        let bundled = doc.nodes[id].stylesheet.is_some()
            || matches!(doc.nodes[id].children[..], [child] if doc.nodes[child].stylesheet.is_some());
        self.cssom.insert(
            id,
            Cssom {
                used: self.pass,
                text: text.into(),
                bundled,
                disabled,
                sheet,
            },
        );
        result
    }

    pub(crate) fn finish(&mut self) {
        let pass = self.pass;
        let count = self.scopes.len();
        self.scopes.retain(|scope| scope.used == pass);
        if self.scopes.len() < count {
            let live: rustc_hash::FxHashSet<_> = self.scopes.iter().map(|scope| scope.id).collect();
            self.values.retain(|key, _| live.contains(&key.scope));
            self.queries.retain(|key, _| live.contains(&key.0));
            self.matches.forget_selectors();
        }
        self.sheets.retain(|_, (used, ..)| *used == pass);
        self.tables.retain(|_, table| table.pass == pass);
        if self
            .walk
            .as_ref()
            .is_some_and(|walk| walk.used.get() != pass)
        {
            self.walk = None;
        }
        // A filtered list is also the source of its own entry, so dropping one entry can free another.
        loop {
            let count = self.inheritable.len();
            self.inheritable
                .retain(|_, (from, _)| Rc::strong_count(from) > 1);
            if self.inheritable.len() == count {
                break;
            }
        }
        self.derived.clear();
        self.heritage
            .retain(|_, (style, _)| Rc::strong_count(style) > 1);
        self.inherited.retain(|style| Rc::strong_count(style) > 1);
        self.cssom.retain(|_, edited| edited.used >= pass);
        // Incremental passes don't visit clean nodes, so values that nodes still hold stay.
        if self.values.len() > 4096 {
            self.values.retain(|_, values| {
                values.retain(|value| value.used.get() == pass || Rc::strong_count(value) > 1);
                !values.is_empty()
            });
        }
        self.states = self
            .scopes
            .iter()
            .fold(0, |states, scope| states | scope.states);
        self.pass += 1;
    }
}

#[derive(Default)]
pub(crate) struct StyleContext {
    pub shadows: Rc<Shadows>,
    pub top_layer: bool,
    pub animated: HashMap<usize, Style>,
    pub reduced_motion: bool,
    pub theme: u32,
    pub scale: f32,
    pub aliases: HashMap<String, String>,
    pub inserted: Vec<(Rc<Stylesheet>, bool)>,
    pub previous: Option<Rc<ComputedStyles>>,
}

#[derive(Default)]
pub(crate) struct Shadows {
    pub roots: FxHashMap<usize, usize>,
    pub hosts: Rc<FxHashMap<usize, usize>>,
    pub composed: FxHashMap<usize, Vec<usize>>,
}

#[derive(Default)]
pub(crate) struct TreeWalk {
    pub(crate) walk: crate::dom::Walk,
    pub(crate) built: (u64, Rc<Shadows>),
    pub(crate) used: std::cell::Cell<u64>,
    pub(crate) tree: Rc<Vec<u32>>,
    pub(crate) trees: Rc<Vec<(usize, usize)>>,
    pub(crate) special: Vec<Vec<usize>>,
    pub(crate) tables: Vec<usize>,
}

pub(crate) fn compute_styles(
    doc: &Document,
    width: f32,
    height: f32,
    text_system: Option<&renderer::TextSystem>,
    cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
) -> ComputedStyles {
    let mut out = ComputedStyles::default();
    // The generation and revision of each dependency key in the list of the current node.
    let (mut unique, mut generation) = (Vec::<(u32, u32)>::new(), 0u32);
    let (reduced_motion, native_theme) = context.map_or_else(
        || (false, 0),
        |context| (context.reduced_motion, context.theme),
    );
    if let Some(text) = text_system {
        font_names(
            text,
            context.map_or(&HashMap::new(), |context| &context.aliases),
        );
    }
    let blank = || Collect {
        width,
        height,
        reduced_motion,
        native_theme,
        rules: Vec::new(),
        container: None,
        media: None,
        blocks: Vec::new(),
        layers: Vec::new(),
        keyframes: HashMap::new(),
        properties: Registry::default(),
        layered: HashMap::new(),
    };
    let shadows = context.map_or_else(Default::default, |context| context.shadows.clone());
    style_cache.shadows = shadows.clone();
    let walk = style_cache.walk(doc, &shadows);
    let order = walk.walk.nodes.as_slice();
    let stored = context.is_some_and(|context| context.animated.is_empty());
    let previous = context
        .and_then(|context| context.previous.as_deref())
        .filter(|previous| {
            previous.values.len() <= doc.nodes.len() && previous.lengths.list_len() <= 4096
        });
    let mut fast = previous.is_some() && !style_cache.full;
    // This scan relies on the page setting `top_layer` whenever a node is fullscreen.
    let fullscreen = context.is_none_or(|context| context.top_layer)
        && order
            .iter()
            .any(|&(id, _)| doc.nodes[id as usize].state & crate::dom::FULLSCREEN != 0);
    let inserted = context.map_or(&[][..], |context| &context.inserted[..]);
    let _user = |sheet: &Rc<Stylesheet>| {
        inserted
            .iter()
            .any(|(inserted, user)| *user && Rc::ptr_eq(inserted, sheet))
    };
    let scale = context.map_or(1., |context| context.scale);
    let metrics = text_system.map(|_| scale);
    let old = &mut style_cache.environment;
    if old.viewport != [width, height]
        || old.metrics != metrics
        || old.reduced_motion != reduced_motion
        || old.native_theme != native_theme
    {
        fast = false;
        style_cache.full = true;
        let resized = old.metrics == metrics
            && old.reduced_motion == reduced_motion
            && old.native_theme == native_theme;
        old.viewport = [width, height];
        old.metrics = metrics;
        old.reduced_motion = reduced_motion;
        old.native_theme = native_theme;
        style_cache.values.retain(|_, values| {
            values.retain(|value| resized && !value.viewport);
            !values.is_empty()
        });
        if !resized {
            style_cache.queries.clear();
        }
        // Scopes can share `@media` blocks, so set new results only after every scope has checked them.
        let probe = blank();
        let mut flipped: Vec<Rc<MediaBlock>> = Vec::new();
        for block in style_cache.scopes.iter().flat_map(|scope| &scope.blocks) {
            if probe.media(&block.queries) != block.matched.get()
                && !flipped.iter().any(|old| Rc::ptr_eq(old, block))
            {
                flipped.push(block.clone());
            }
        }
        let hit = |stack: &Option<MediaStack>| {
            stack.as_ref().is_some_and(|stack| {
                stack
                    .iter()
                    .any(|block| flipped.iter().any(|old| Rc::ptr_eq(old, block)))
            })
        };
        let count = style_cache.scopes.len();
        style_cache.scopes.retain_mut(|scope| {
            if scope
                .blocks
                .iter()
                .any(|block| block.fixed && flipped.iter().any(|old| Rc::ptr_eq(old, block)))
            {
                return false;
            }
            let StyleScope {
                nodes,
                medias,
                pseudos,
                ..
            } = scope;
            for node in nodes.iter_mut().flatten() {
                if !resized
                    || node
                        .cached
                        .as_ref()
                        .is_some_and(|cached| cached.value.viewport)
                    || !flipped.is_empty()
                        && (node.rules.iter().any(|(order, _)| hit(&medias[*order]))
                            || node
                                .pseudos
                                .iter()
                                .any(|&index| hit(&medias[pseudos[index].2])))
                {
                    node.cached = None;
                }
            }
            true
        });
        for block in &flipped {
            block.matched.set(!block.matched.get());
        }
        if style_cache.scopes.len() < count {
            style_cache.matches.forget_selectors();
        }
        let live: rustc_hash::FxHashSet<_> =
            style_cache.scopes.iter().map(|scope| scope.id).collect();
        style_cache.queries.retain(|key, _| live.contains(&key.0));
    }
    let (chosen, scheme, theme) = style_cache.choose_scopes(
        doc,
        &walk,
        cache,
        inserted,
        fullscreen,
        &mut fast,
        [width, height],
        reduced_motion,
        native_theme,
    );
    let indices: FxHashMap<usize, usize> = style_cache
        .scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| (scope.id, index))
        .collect();
    let scopes: Vec<usize> = chosen
        .iter()
        .map(|id| indices.get(id).copied().unwrap_or_default())
        .collect();
    {
        let mut registry = style_cache.scopes[scopes[0]].properties.clone();
        registry.extend(
            style_cache
                .scripted
                .iter()
                .map(|(&prop, value)| (prop, value.clone())),
        );
        if registry != *style_cache.environment.registry {
            let old = &*style_cache.environment.registry;
            let changed: rustc_hash::FxHashSet<Prop> = registry
                .iter()
                .filter(|(prop, value)| old.get(prop) != Some(value))
                .map(|(prop, _)| *prop)
                .chain(
                    old.keys()
                        .filter(|prop| !registry.contains_key(prop))
                        .copied(),
                )
                .collect();
            // A registration changes only the styles that hold the name or read it, as `var()` reads initial values.
            let holds = |vars: &Vars| vars.iter().any(|(prop, _)| changed.contains(prop));
            let touched =
                |value: &CachedStyle| {
                    let vars = value.vars.borrow();
                    value.animations.is_some()
                        || value.reads.as_ref().is_none_or(|reads| {
                            reads.iter().any(|(prop, _)| changed.contains(prop))
                        })
                        || value.missing.iter().any(|name| {
                            Prop::find(name).is_some_and(|prop| changed.contains(&prop))
                        })
                        || vars.0.as_ref().is_some_and(holds)
                        || holds(&vars.1)
                };
            style_cache.values.retain(|_, values| {
                values.retain(|value| !touched(value));
                !values.is_empty()
            });
            for scope in &mut style_cache.scopes {
                for (id, node) in scope.nodes.iter_mut().enumerate() {
                    let Some(node) = node else { continue };
                    if node.cached.as_ref().is_none_or(|cached| {
                        touched(&cached.value)
                            || cached.inherited.as_ref().is_some_and(holds)
                            || holds(&cached.vars)
                    }) {
                        node.cached = None;
                        style_cache.matches.mark(id, false);
                    }
                }
            }
            style_cache.environment.registry = Rc::new(registry);
            style_cache.inheritable.clear();
            if let Some(styled) = &mut style_cache.styled {
                styled.environment.registry = style_cache.environment.registry.clone();
            }
        }
    }
    if scheme != style_cache.environment.scheme {
        style_cache.environment.scheme = scheme.clone();
        style_cache.uncache();
    }
    let environment = style_cache.environment.clone();
    let registry = environment.registry.clone();
    let mut used: Vec<(usize, usize, usize)> = walk
        .trees
        .iter()
        .zip(&scopes)
        .map(|(&(root, _), &index)| {
            let scope = &style_cache.scopes[index];
            (root, scope.id, scope.rules.len())
        })
        .collect();
    used.sort_unstable();
    // Reuse previous only with the same viewport, registry and scopes. Unstored passes can change these too.
    fast &= style_cache.styled.as_ref().is_some_and(|old| {
        old.environment == environment
            && used.iter().all(|scope| {
                match old.scopes.binary_search_by_key(&scope.0, |old| old.0) {
                    Ok(index) => old.scopes[index] == *scope,
                    Err(_) => true,
                }
            })
    });
    out.registry = registry.clone();
    out.theme = native_theme;
    let count = doc.nodes.len();
    match previous.filter(|_| fast) {
        Some(previous) => {
            out.values.clone_from(&previous.values);
            out.vars.clone_from(&previous.vars);
            out.flags.clone_from(&previous.flags);
            out.lengths.clone_from(&previous.lengths);
            out.lengths.grow(count);
            out.pseudos.clone_from(&previous.pseudos);
            out.text_pseudos.clone_from(&previous.text_pseudos);
            out.backdrops.clone_from(&previous.backdrops);
            out.backdrop_starts.clone_from(&previous.backdrop_starts);
            out.counting.clone_from(&previous.counting);
            out.scrollbars.clone_from(&previous.scrollbars);
            out.selections.clone_from(&previous.selections);
            out.animations.clone_from(&previous.animations);
            out.important.clone_from(&previous.important);
            out.starting.clone_from(&previous.starting);
            out.containers.clone_from(&previous.containers);
            out.slotted.clone_from(&previous.slotted);
            out.parts.clone_from(&previous.parts);
            out.exported.clone_from(&previous.exported);
        }
        None => out.lengths.reset(count, Lengths::new(16., width, height)),
    }
    let (style, vars) = empty();
    out.values.resize(count, style);
    out.vars.resize(count, vars);
    out.flags.resize(count, 0);
    out.containers.resize(count, None);
    let mut orphans = std::mem::take(&mut style_cache.orphans);
    match previous.filter(|_| fast) {
        // The cascade must read the own styles of the nodes that the last anonymous tables changed.
        Some(previous) => {
            for (&id, &flags) in &previous.anonymous {
                if let Some(own) = previous.authored.get(&id).filter(|_| id < count) {
                    out.values[id] = own.clone();
                    out.flags[id] = flags;
                }
            }
        }
        None => orphans.clear(),
    }
    let mut inheritable = std::mem::take(&mut style_cache.inheritable);
    let mut local: Vec<Prop> = registry
        .iter()
        .filter(|(_, (_, inherits, _))| !inherits)
        .map(|(&prop, _)| prop)
        .collect();
    local.sort_unstable();
    let mut inherit = |vars: &Vars| -> Vars {
        if local.is_empty() {
            return vars.clone();
        }
        inheritable
            .entry(Rc::as_ptr(vars))
            .or_insert_with(|| {
                // Both lists are in order. Most lists hold no local property, so they are not copied.
                let (mut rest, mut kept) = (&local[..], None::<Vec<_>>);
                for (index, entry) in vars.iter().enumerate() {
                    while rest.first().is_some_and(|&prop| prop < entry.0) {
                        rest = &rest[1..];
                    }
                    if rest.first() == Some(&entry.0) {
                        kept.get_or_insert_with(|| vars[..index].to_vec());
                    } else if let Some(kept) = &mut kept {
                        kept.push(entry.clone());
                    }
                }
                (vars.clone(), kept.map(Rc::new))
            })
            .1
            .clone()
            .unwrap_or_else(|| vars.clone())
    };
    let (unstyled, empty) = empty();
    let matches = &mut style_cache.matches;
    matches.set_hosts(&shadows.hosts);
    matches.update(doc, &walk.walk);
    let mut candidates: Vec<usize> = Vec::new();
    let mut shares: FxHashMap<(usize, [u32; 7]), Share> = FxHashMap::default();
    let no_pseudos: Rc<[usize]> = Rc::new([]);
    let unshared = style_cache.unshared;
    let ComputedStyles {
        values: styles,
        vars: custom,
        flags: style_flags,
        lengths,
        pseudos,
        text_pseudos,
        backdrops,
        backdrop_starts,
        scrollbars,
        selections,
        animations,
        important,
        slotted,
        parts,
        exported,
        starting: starts,
        list_items: _,
        counting,
        authored: _,
        anonymous: _,
        containers,
        counted: _,
        revision,
        ..
    } = &mut out;
    *revision = doc.nodes.latest();
    macro_rules! unstyle {
        ($id:expr) => {
            pseudos.remove(&$id);
            text_pseudos.remove(&$id);
            backdrops.remove(&$id);
            backdrop_starts.remove(&$id);
            counting.remove(&$id);
            scrollbars.remove(&$id);
            selections.remove(&$id);
            animations.remove(&$id);
            important.remove(&$id);
            starts.remove(&$id);
            slotted.remove(&$id);
            parts.remove(&$id);
            exported.remove(&$id);
        };
    }
    const CHANGED: u8 = 1;
    const DEEP: u8 = 2;
    let mut changed = std::mem::take(&mut style_cache.changed);
    style_cache.stamp += 1;
    let stamp = style_cache.stamp << 8;
    macro_rules! changed {
        ($id:expr) => {
            match changed[$id] ^ stamp {
                value if value < 256 => value as u8,
                _ => 0,
            }
        };
    }
    let mut counters = false;
    // A fast pass numbers the counters again only when one of their inputs changed.
    let mut recount = false;
    let html: Vec<usize> = walk.special[0]
        .iter()
        .copied()
        .filter(|&id| doc.nodes[id].tag == "html")
        .collect();
    if fast {
        if changed.len() < doc.nodes.len() {
            changed.resize(doc.nodes.len(), 0);
        }
        if let Some(old) = style_cache
            .styled
            .as_ref()
            .filter(|old| !Rc::ptr_eq(&old.nodes, &walk.walk.nodes))
        {
            // Text and comments that move need no new count.
            let changed = walk.walk.changed_since(old.serial).map_or(0, <[u32]>::len);
            recount = walk.walk.since(old.serial).is_none_or(|touched| {
                touched[..touched.len() - changed].iter().any(|&id| {
                    doc.nodes
                        .get(id as usize)
                        .is_none_or(|node| !node.tag.starts_with('#'))
                })
            });
            const GONE: u32 = u32::MAX - 1;
            const KEPT: u32 = u32::MAX - 2;
            let root = |tree: &[u32], trees: &[(usize, usize)], id: usize| {
                tree.get(id)
                    .and_then(|&index| trees.get(index as usize))
                    .map(|tree| tree.0)
            };
            macro_rules! leave {
                ($id:expr) => {
                    styles[$id] = unstyled.clone();
                    custom[$id] = empty.clone();
                    style_flags[$id] = 0;
                    lengths.unset($id);
                    containers[$id] = None;
                    unstyle!($id);
                };
            }
            // Without shadow trees, only the nodes that a patched walk touched can have another place.
            let touched = walk
                .walk
                .since(old.serial)
                .filter(|_| shadows.hosts.is_empty() && old.trees.len() == 1);
            let mut placed = Vec::new();
            if touched.is_none() || cfg!(debug_assertions) {
                placed.resize(doc.nodes.len(), GONE);
                for &(id, up) in old.nodes.iter() {
                    if let Some(slot) = placed.get_mut(id as usize) {
                        *slot = up;
                    }
                }
            }
            match touched {
                Some(touched) => {
                    // A node with another parent is dirty by its snapshot. A node that joins has no style.
                    for &id in touched {
                        let id = id as usize;
                        if !walk.walk.contains(id) {
                            leave!(id);
                        } else if Rc::ptr_eq(&styles[id], &unstyled) {
                            matches.dirty[id] = true;
                        }
                    }
                    #[cfg(debug_assertions)]
                    for (nodes, kept) in [(order, true), (&old.nodes[..], false)] {
                        for &(id, up) in nodes {
                            let slot = &mut placed[id as usize];
                            assert!(
                                *slot == if kept { up } else { KEPT } || touched.contains(&id),
                                "node {id} moved outside the patched walk"
                            );
                            assert!(
                                !kept
                                    || *slot == up
                                    || matches.dirty[id as usize]
                                    || doc.nodes[id as usize].parent != Some(up as usize),
                                "node {id} moved without a dirty mark"
                            );
                            *slot = if kept { KEPT } else { *slot };
                        }
                    }
                }
                None => {
                    for &(id, up) in order {
                        let id = id as usize;
                        if placed[id] != up
                            || root(&old.tree, &old.trees, id) != root(&walk.tree, &walk.trees, id)
                        {
                            matches.dirty[id] = true;
                            if let Some(&host) = shadows.hosts.get(&id) {
                                matches.dirty[host] = true;
                            }
                        }
                        placed[id] = KEPT;
                    }
                    for &(id, _) in old.nodes.iter() {
                        let id = id as usize;
                        if placed.get(id).is_some_and(|&slot| slot != KEPT) {
                            leave!(id);
                        }
                    }
                }
            }
        }
    }
    // Named `style()` queries put the values of containers into the chain, so all chains change with them.
    let named_queries = style_cache.scopes.iter().any(|scope| scope.named);
    let mut all = std::mem::replace(&mut style_cache.named, named_queries) != named_queries;
    let mut root = FontUnits::new(16.);
    let mut font_metrics = HashMap::new();
    let mut passed_on = FxHashMap::default();
    let mut merges = FxHashMap::default();
    let last = context.and_then(|context| context.previous.as_deref());
    let address =
        |list: Option<&Option<Rc<Container>>>| list.map(|list| list.as_ref().map(Rc::as_ptr));
    let mut hosted: Vec<Passed> = Vec::new();
    let mut texts = FxHashMap::<_, (Rc<Style>, Vars, u16, Lengths, Option<Rc<Style>>)>::default();
    macro_rules! settle {
        (@changed $id:expr, $up:expr) => {
            let (id, up): (usize, Option<usize>) = ($id, $up);
            if shadows.roots.contains_key(&id) {
                let list = exported_parts(
                    std::mem::take(&mut hosted),
                    parts.get(&id),
                    exported.get(&walk.trees[walk.tree[id] as usize].1),
                    doc.nodes[id].attrs.get("exportparts"),
                    last.and_then(|last| last.exported.get(&id)),
                );
                match list {
                    Some(list) => exported.insert(id, list),
                    None => exported.remove(&id),
                };
            }
            if let Some(previous) = previous.filter(|_| fast) {
                let font = previous.lengths.get(id).map(|old| old.font);
                let pointer = |list: Option<&Shadowed>| list.map(Rc::as_ptr);
                changed[id] = stamp | u64::from(u8::from(
                    !(previous
                        .authored
                        .get(&id)
                        .or(previous.values.get(id))
                        .is_some_and(|old| Rc::ptr_eq(old, &styles[id]))
                        && previous
                            .vars
                            .get(id)
                            .is_some_and(|old| Rc::ptr_eq(old, &custom[id]))
                        && font == Some(lengths[id].font)
                        && address(previous.containers.get(id)) == address(containers.get(id))
                        && previous.selections.get(&id) == selections.get(&id)
                        && pointer(previous.slotted.get(&id)) == pointer(slotted.get(&id))),
                ) * CHANGED
                    | up.map_or(0, |up| changed!(up) & DEEP)
                    | u8::from(pointer(previous.exported.get(&id)) != pointer(exported.get(&id)))
                        * DEEP);
                all |= (id == 0 || html.contains(&id)) && font != Some(root);
                if !recount {
                    let old = previous.anonymous.get(&id).or(previous.flags.get(id));
                    let old = old.copied().unwrap_or(0);
                    let before = previous.authored.get(&id).or(previous.values.get(id));
                    recount = (old | style_flags[id]) & COUNTED != 0
                        || (old ^ style_flags[id]) & DISPLAY_NONE != 0
                        || listed(doc, id)
                        || list_item(&styles[id])
                        || before.is_some_and(|before| list_item(before));
                }
            }
            counters |= style_flags[id] & COUNTED != 0;
            if matches!(style_flags[id] >> 8, CELL | CAPTION | 10..=12)
                && let Some(up) = up.filter(|&up| style_flags[up] >> 8 == 0)
            {
                orphans.push(up);
            }
        };
        ($id:expr, $up:expr, $seen:expr) => {
            let id = $id;
            let link = match Container::of(id, &styles[id], named_queries.then(|| &custom[id]), $seen.clone()) {
                Some(link) => match last.and_then(|last| last.containers.get(id)?.as_ref()) {
                    Some(old) if **old == link => Some(old.clone()),
                    _ => Some(Rc::new(link)),
                },
                None => $seen,
            };
            containers.set(id, link, |a, b| address(Some(a)) == address(Some(b)));
            settle!(@changed id, $up);
        };
    }
    // A pass with animated values must leave the dirty marks, as the next stored pass starts again from the
    // last stored one.
    let mut moving = Vec::new();
    if fast && !stored {
        moving.resize(doc.nodes.len(), false);
        for &id in context
            .into_iter()
            .flat_map(|context| context.animated.keys())
        {
            if let Some(slot) = moving.get_mut(id) {
                *slot = true;
            }
        }
    }
    // A fast pass visits only the dirty and moving nodes and the nodes below a changed one. The document and
    // the root elements are in the list because they set the root font size.
    let mut seeds: Vec<u32> = Vec::new();
    if fast {
        let dirty = matches.dirty.chunks(64).enumerate();
        let dirty = dirty.filter(|(_, chunk)| chunk.contains(&true));
        let dirty = dirty.flat_map(|(index, chunk)| {
            let set = chunk.iter().enumerate().filter(|(_, dirty)| **dirty);
            set.map(move |(bit, _)| index * 64 + bit)
        });
        let animated = context.filter(|_| !stored);
        let animated = animated
            .into_iter()
            .flat_map(|context| context.animated.keys().copied());
        let roots = std::iter::once(0).chain(html.iter().copied());
        let places = roots.chain(dirty).chain(animated);
        seeds = places.filter_map(|id| walk.walk.place(id)).collect();
        seeds.sort_unstable();
    }
    let (mut next, mut seed, mut scan) = (0, 0, 0);
    let mut visited: Option<(usize, usize)> = None;
    loop {
        if let Some((at, id)) = visited.take()
            && changed!(id) != 0
        {
            scan = scan.max(at + walk.walk.span(id));
        }
        let mut reached = true;
        if fast && !all && next >= scan {
            while seeds.get(seed).is_some_and(|&at| (at as usize) < next) {
                seed += 1;
            }
            // Debug builds visit every node, to check that the skipped ones need no new style.
            match seeds.get(seed) {
                _ if cfg!(debug_assertions) => reached = seeds.get(seed) == Some(&(next as u32)),
                Some(&at) => next = at as usize,
                None => break,
            }
        }
        let Some(&(id, up)) = order.get(next) else {
            break;
        };
        let at = next;
        next += 1;
        let id = id as usize;
        if fast {
            if !all
                && !matches.dirty[id]
                && moving.get(id) != Some(&true)
                && (up == u32::MAX || changed!(up as usize) == 0)
            {
                if id == 0 || html.contains(&id) {
                    root = lengths[id].font;
                }
                continue;
            }
            assert!(
                reached || all,
                "node {id} needs a style outside the dirty list"
            );
            visited = Some((at, id));
            if stored {
                matches.dirty[id] = false;
            }
            unstyle!(id);
        }
        let up = (up != u32::MAX).then_some(up as usize);
        // A patched walk without shadow trees leaves `tree` as it was.
        let tree = match walk.trees.len() {
            1 => 0,
            _ => walk.tree[id] as usize,
        };
        let node = &doc.nodes[id];
        if tree != 0 && walk.trees[tree].0 == id {
            let host = walk.trees[tree].1;
            styles.set(id, styles[host].clone(), Rc::ptr_eq);
            custom.set(id, custom[host].clone(), Rc::ptr_eq);
            style_flags.set(id, style_flags[host], u16::eq);
            lengths.set(id, lengths[host]);
            containers.set(id, containers[host].clone(), |a, b| {
                address(Some(a)) == address(Some(b))
            });
            if let Some(selection) = selections.get(&host).cloned() {
                selections.insert(id, selection);
            }
            settle!(@changed id, up);
            continue;
        }
        let seen = up.and_then(|up| containers[up].clone());
        let parent_style = up.map(|up| &styles[up]);
        // Children of a `display: contents` box are items of the nearest ancestor that has a box.
        let parent_role = up.map_or(0, |up| style_flags[up] >> 8);
        let (item, table_item) = {
            let (mut at, mut display) = (
                up,
                parent_style.and_then(|parent| parent.get(Prop::DISPLAY)),
            );
            while display == Some("contents") {
                at = at
                    .and_then(|at| walk.walk.parent(at))
                    .filter(|&at| at != u32::MAX)
                    .map(|at| at as usize);
                display = at.and_then(|at| styles[at].get(Prop::DISPLAY));
            }
            (
                matches!(
                    display,
                    Some("flex" | "inline-flex" | "grid" | "inline-grid")
                ) || at.is_some_and(|at| drawable_canvas(&doc.nodes[at], &styles[at])),
                at.is_some_and(|at| table_box(style_flags[at] >> 8)),
            )
        };
        let parent_vars = up
            .map(|up| &custom[up])
            .filter(|_| !matches!(node.tag.as_str(), "#text" | "#comment"));
        // Custom properties of the parent that `style()` queries read.
        let style_reads = std::cell::RefCell::new(Vec::new());
        let parent = up.map_or(FontUnits::new(16.), |up| lengths[up].font);
        let inputs = Inputs::new(parent, root);
        let mut waiting: Vec<(usize, usize, ContainerQuery)> = Vec::new();
        let mut shadow: Vec<ShadowRule> = Vec::new();
        let mut contained =
            |rule: &mut ShadowRule, key: (usize, usize), conditions: &ContainerQuery| {
                let applies = query_containers(
                    conditions,
                    seen.as_deref(),
                    key,
                    &style_cache.containers,
                    |container| lengths[container],
                    &mut style_cache.queries,
                    (parent_vars, &style_reads),
                );
                if matches!(rule.kind, ELEMENT | STARTING) {
                    return applies;
                }
                waiting.push((key.0, key.1, conditions.clone()));
                rule.query = Some((key.0, key.1, applies));
                true
            };
        hosted.clear();
        if let Some(&root) = shadows.roots.get(&id)
            && let Some(&index) = walk
                .tree
                .get(root)
                .and_then(|&tree| scopes.get(tree as usize))
        {
            let inner = &mut style_cache.scopes[index];
            if !inner.nodes[root]
                .as_ref()
                .is_some_and(|matched| matches.valid_since(&matched.dependencies, &matched.checked))
            {
                matches.dependencies.clear();
                matches.depend(root, 0);
                let mut matched: Vec<(usize, [usize; 3])> = Vec::new();
                for &entry in inner.universal.iter().filter(|&&entry| entry < PSEUDO) {
                    let (order, index) = (entry >> SELECTOR, entry & ((1 << SELECTOR) - 1));
                    let selector = &inner.rules[order].0[index];
                    if selector.matches_cached(doc, root, None, matches) {
                        match matched.last_mut() {
                            Some(last) if last.0 == order => {
                                last.1 = last.1.max(selector.specificity)
                            }
                            _ => matched.push((order, selector.specificity)),
                        }
                    }
                }
                let pseudos = inner
                    .universal
                    .iter()
                    .filter(|&&index| index >= PSEUDO)
                    .map(|&index| index & !PSEUDO)
                    .filter(|&index| {
                        let (kind, selector, _, key) = &inner.pseudos[index];
                        key.is_empty()
                            && *kind != 12
                            && selector.matches_cached(doc, root, None, matches)
                    })
                    .collect();
                matches.dependencies.sort_unstable();
                matches.dependencies.dedup();
                matches.register(id);
                inner.nodes[root] = Some(MatchedRules {
                    checked: std::cell::Cell::new(matches.epoch),
                    cached: None,
                    dependencies: matches.dependencies.as_slice().into(),
                    rules: matched.into(),
                    pseudos,
                });
            }
            let inner = &*inner;
            for (kind, order, specificity, pseudo) in inner.nodes[root].iter().flat_map(|matched| {
                matched
                    .rules
                    .iter()
                    .map(|&(order, specificity)| (ELEMENT, order, specificity, 0))
                    .chain(matched.pseudos.iter().map(|&index| {
                        let (kind, selector, order, _) = &inner.pseudos[index];
                        (*kind, *order, selector.specificity, index)
                    }))
            }) {
                if inner.medias[order]
                    .as_ref()
                    .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                {
                    continue;
                }
                let (_, declarations, layer) = &inner.rules[order];
                let mut rule = ShadowRule {
                    kind,
                    context: -1,
                    layer: *layer,
                    specificity,
                    order: inner.keys[order],
                    declarations: declarations.clone(),
                    query: None,
                };
                if kind == PART {
                    if let Some((name, compound, target)) = inner.arguments.get(&pseudo) {
                        rule.kind = *target;
                        rule.context = 0;
                        hosted.push(Passed {
                            name: name.clone(),
                            compound: compound.clone(),
                            conditions: inner.containers[order]
                                .clone()
                                .map(|conditions| (inner.id, order, conditions)),
                            rule,
                        });
                    }
                    continue;
                }
                if let Some(conditions) = &inner.containers[order]
                    && !contained(&mut rule, (inner.id, order), conditions)
                {
                    continue;
                }
                shadow.push(rule);
            }
        }
        let selection = up.and_then(|up| selections.get(&up));
        let assigned = up
            .filter(|&up| node.parent != Some(up))
            .and_then(|up| slotted.get(&up));
        let names = (tree != 0).then(|| node.attrs.get("part")).flatten();
        let named = names.and_then(|_| exported.get(&walk.trees[tree].1));
        if assigned.is_some() || named.is_some() {
            matches.dependencies.clear();
            for passed in assigned.into_iter().flat_map(|list| list.iter()).chain(
                named
                    .into_iter()
                    .flat_map(|list| list.iter())
                    .filter(|passed| {
                        names.is_some_and(|names| {
                            !passed.name.is_empty()
                                && passed.name.split_ascii_whitespace().all(|name| {
                                    names.split_ascii_whitespace().any(|part| part == name)
                                })
                        })
                    }),
            ) {
                if !passed
                    .compound
                    .iter()
                    .any(|selector| selector.matches_cached(doc, id, None, matches))
                {
                    continue;
                }
                let mut rule = passed.rule.clone();
                if let Some((scope, order, conditions)) = &passed.conditions
                    && !contained(&mut rule, (*scope, *order), conditions)
                {
                    continue;
                }
                shadow.push(rule);
            }
            matches.dependencies.sort_unstable();
            matches.dependencies.dedup();
            matches.register(id);
        }
        let shadow: Option<Rc<[ShadowRule]>> = (!shadow.is_empty()).then(|| shadow.into());
        let animated = context.and_then(|context| context.animated.get(&id));
        let text = (matches!(node.tag.as_str(), "#text" | "#comment")
            && shadow.is_none()
            && animated.is_none()
            && !unshared)
            .then(|| {
                (
                    scopes[tree],
                    node.tag == "#comment",
                    parent_style.map(Rc::as_ptr),
                    inputs.units,
                    selection.map(Rc::as_ptr),
                )
            });
        if let Some((style, vars, flags, units, selected)) =
            text.as_ref().and_then(|text| texts.get(text))
        {
            styles.set(id, style.clone(), Rc::ptr_eq);
            custom.set(id, vars.clone(), Rc::ptr_eq);
            style_flags.set(id, *flags, u16::eq);
            lengths.set(id, *units);
            if let Some(selected) = selected {
                selections.insert(id, selected.clone());
            }
            settle!(id, up, seen);
            continue;
        }
        let scoped = &mut style_cache.scopes[scopes[tree]];
        let (rules, keys, scope) = (&scoped.rules, &scoped.keys, scoped.id);
        let mut key = None;
        let mut passed = None;
        let mut written = None;
        let mut moved = false;
        let mut contained = false;
        let mut reuse =
            |value: &CachedStyle, old: Option<&Vars>, vars: &Vars| match (old, parent_vars) {
                (None, None) => Some((vars.clone(), false)),
                (Some(old), Some(new)) if Rc::ptr_eq(old, new) => Some((vars.clone(), false)),
                (Some(_), Some(new)) => {
                    let reads = value.reads.as_ref()?;
                    (value
                        .missing
                        .iter()
                        .all(|name| Prop::find(name).is_none_or(|prop| var(new, prop).is_none()))
                        && reads
                            .iter()
                            .all(|(prop, value)| var(new, *prop) == value.as_ref()))
                    .then(|| {
                        if value.own.is_empty() {
                            return (inherit(new), true);
                        }
                        // Nodes with one style and one parent list must share the new list, or the
                        // nodes below them cannot share their styles.
                        let key = (Rc::as_ptr(new), value.own.as_ptr());
                        let merge = || {
                            let vars = inherit(new);
                            if value
                                .own
                                .iter()
                                .all(|(prop, own)| var(&vars, *prop) == own.as_ref())
                            {
                                return vars;
                            }
                            let mut list = (*vars).clone();
                            for (prop, own) in &value.own {
                                match (list.binary_search_by_key(prop, |entry| entry.0), own) {
                                    (Ok(index), Some(own)) => list[index].1 = own.clone(),
                                    (Err(index), Some(own)) => {
                                        list.insert(index, (*prop, own.clone()))
                                    }
                                    (Ok(index), None) => {
                                        list.remove(index);
                                    }
                                    (Err(_), None) => {}
                                }
                            }
                            Rc::new(list)
                        };
                        (merges.entry(key).or_insert_with(merge).clone(), true)
                    })
                }
                _ => None,
            };
        let cached = scoped.nodes[id]
            .as_ref()
            .filter(|node| {
                animated.is_none() && matches.valid_since(&node.dependencies, &node.checked)
            })
            .and_then(|node| node.cached.as_ref())
            .filter(|cached| {
                let same = match (&cached.parent, parent_style) {
                    (None, None) => true,
                    (Some(old), Some(new)) => {
                        Rc::ptr_eq(&old.0, new)
                            || !cached.value.keyword
                                && *passed_on
                                    .entry((Rc::as_ptr(&old.0), Rc::as_ptr(new)))
                                    .or_insert_with(|| {
                                        let own = |style: &'_ Rc<Style>| {
                                            style
                                                .own()
                                                .filter(|(prop, _)| prop.inherited())
                                                .map(|(prop, value)| (prop, value.clone()))
                                                .collect::<Vec<_>>()
                                        };
                                        // A `contents` box hands down the display of its ancestors.
                                        old.0.get(Prop::DISPLAY) == new.get(Prop::DISPLAY)
                                            && new.get(Prop::DISPLAY) != Some("contents")
                                            && own(&old.0) == own(new)
                                    })
                    }
                    _ => false,
                };
                same && cached.units == inputs.hash()
                    && cached
                        .containers
                        .as_ref()
                        .is_none_or(|containers| *containers == seen)
                    && cached.same_rare(shadow.as_ref(), selection)
            })
            .and_then(|cached| {
                let (vars, changed) =
                    reuse(&cached.value, cached.inherited.as_ref(), &cached.vars)?;
                moved = changed;
                Some((cached.value.clone(), vars))
            })
            .or_else(|| {
                let group = (!unshared && !node.tag.starts_with('#')).then(|| {
                    let [own, up] = [Some(id), node.parent]
                        .map(|id| id.map(|id| matches.signature(id)).unwrap_or_default());
                    (scope, [own[0], own[3], own[5], own[7], up[0], up[5], up[8]])
                });
                let (matched, matched_pseudos) = if let Some(cached) = scoped.nodes[id].as_ref()
                    && matches.valid_since(&cached.dependencies, &cached.checked)
                {
                    (cached.rules.clone(), cached.pseudos.clone())
                } else if node.tag.starts_with('#') {
                    let none = intern(&mut scoped.lists, &[]);
                    scoped.nodes[id] = Some(MatchedRules {
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
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: matches.dependencies.as_slice().into(),
                        rules: share.rules.clone(),
                        pseudos: share.pseudos.clone(),
                    });
                    (share.rules.clone(), share.pseudos.clone())
                } else {
                    matches.dependencies.clear();
                    // Candidates come from the node's classes, so its matches always read them.
                    matches.depend(id, 0);
                    matches.depend(id, 7);
                    candidates.clear();
                    candidates.extend(&scoped.universal);
                    let classes = node.attrs.get("class").into_iter();
                    // Keys are lowercase. Only SVG names have uppercase letters, so HTML names need no copy.
                    let lower = crate::css::lowercase;
                    for (kind, name) in std::iter::once((2, lower(&node.tag)))
                        .chain(node.attrs.get("id").map(|id| (0, id.as_str().into())))
                        .chain(classes.flat_map(|c| {
                            c.split_ascii_whitespace().map(|class| (1, class.into()))
                        }))
                        .chain(node.attrs.keys().map(|name| (3, lower(name))))
                    {
                        if let Some(list) = scoped.buckets[kind].get(&*name) {
                            candidates.extend(list);
                        }
                    }
                    if !scoped.buckets[4].is_empty() {
                        let mut pair = String::new();
                        for (name, value) in node.attrs.iter() {
                            pair.clear();
                            pair.push_str(&lower(name));
                            pair.push('=');
                            pair.push_str(value);
                            if let Some(list) = scoped.buckets[4].get(pair.as_str()) {
                                candidates.extend(list);
                            }
                        }
                    }
                    let fresh = scoped.nodes[id].is_none();
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
                                    selector.matches_fresh(doc, id, matches)
                                } else {
                                    selector.matches_cached(doc, id, None, matches)
                                }
                    };
                    let (mut matched, mut matched_pseudos) = (Vec::new(), Vec::new());
                    for &entry in candidates.iter() {
                        if entry >= PSEUDO {
                            let index = entry & !PSEUDO;
                            if test(&scoped.pseudos[index].1, matches) {
                                matched_pseudos.push(index);
                            }
                            continue;
                        }
                        let (order, index) = (entry >> SELECTOR, entry & ((1 << SELECTOR) - 1));
                        let selector = &rules[order].0[index];
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
                    // Cells take `cellpadding` from their table.
                    if matches!(node.tag.as_str(), "td" | "th") {
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
                    // Each candidate selector reads the node again, so most entries repeat: drop them before sorting.
                    generation = generation.wrapping_add(1);
                    if generation == 0 {
                        unique.fill((0, 0));
                        generation = 1;
                    }
                    matches.dependencies.retain(|&(key, revision)| {
                        let key = key as usize;
                        if key >= unique.len() {
                            unique.resize(key + 1, (0, 0));
                        }
                        let seen = std::mem::replace(&mut unique[key], (generation, revision));
                        seen != (generation, revision)
                    });
                    matches.dependencies.sort_unstable();
                    matches.register(id);
                    let shared = intern(&mut scoped.lists, &matched);
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
                                dependencies: matches.dependencies.as_slice().into(),
                                style: style.then(|| matches.signature(id)[6]),
                            },
                        );
                    }
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: matches.dependencies.as_slice().into(),
                        rules: shared.clone(),
                        pseudos: matched_pseudos.clone(),
                    });
                    (shared, matched_pseudos)
                };
                let mut applies = |order: usize, passed: bool| {
                    if scoped.medias[order]
                        .as_ref()
                        .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                    {
                        return false;
                    }
                    let Some(conditions) = scoped.containers[order].as_ref().filter(|_| !passed)
                    else {
                        return true;
                    };
                    contained = true;
                    query_containers(
                        conditions,
                        seen.as_deref(),
                        (scope, order),
                        &style_cache.containers,
                        |container| lengths[container],
                        &mut style_cache.queries,
                        (parent_vars, &style_reads),
                    )
                };
                let conditional = |order: usize| {
                    scoped.medias[order].is_some() || scoped.containers[order].is_some()
                };
                let matched = match matched.iter().any(|&(order, _)| conditional(order)) {
                    true => {
                        let kept: Vec<_> = matched
                            .iter()
                            .copied()
                            .filter(|&(order, _)| applies(order, false))
                            .collect();
                        intern(&mut scoped.lists, &kept)
                    }
                    false => matched,
                };
                let matched_pseudos = match matched_pseudos
                    .iter()
                    .any(|&index| conditional(scoped.pseudos[index].2))
                {
                    true => matched_pseudos
                        .iter()
                        .copied()
                        .filter(|&index| {
                            let (kind, _, order, _) = &scoped.pseudos[index];
                            applies(*order, matches!(*kind, SLOTTED | PART))
                        })
                        .collect(),
                    false => matched_pseudos,
                };
                let graphic = matches!(
                    node.tag.as_str(),
                    "g" | "path"
                        | "rect"
                        | "circle"
                        | "ellipse"
                        | "line"
                        | "polyline"
                        | "polygon"
                        | "text"
                        | "use"
                        | "image"
                        | "foreignObject"
                        | "switch"
                );
                let mut hints = Vec::new();
                for (name, value) in node.attrs.iter() {
                    if graphic
                        && name == "transform"
                        && let Some(value) = svg_transform(value)
                    {
                        hints.push((Prop::TRANSFORM, value.into()));
                    } else if node.tag == "svg"
                        && matches!(name.as_ref(), "width" | "height")
                        // As in Chrome, only the outermost `<svg>` takes them as CSS lengths.
                        && !std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                            .any(|parent| doc.nodes[parent].tag == "svg")
                    {
                        // As in Chrome, these are CSS lengths like `1em` or `50%`, below author styles.
                        let value = value.trim();
                        let digits = value
                            .find(|c: char| !c.is_ascii_digit() && c != '.')
                            .unwrap_or(value.len());
                        let unit = value[digits..].to_ascii_lowercase();
                        if value[..digits].parse::<f32>().is_ok()
                            && matches!(
                                unit.as_str(),
                                "" | "%"
                                    | "px"
                                    | "em"
                                    | "rem"
                                    | "ex"
                                    | "ch"
                                    | "vw"
                                    | "vh"
                                    | "vmin"
                                    | "vmax"
                                    | "cm"
                                    | "mm"
                                    | "q"
                                    | "in"
                                    | "pt"
                                    | "pc"
                            )
                        {
                            let prop = if name == "width" {
                                Prop::WIDTH
                            } else {
                                Prop::HEIGHT
                            };
                            hints.push((
                                prop,
                                if unit.is_empty() {
                                    format!("{value}px").into()
                                } else {
                                    value.into()
                                },
                            ));
                        }
                    } else if let Some(prop) = match (node.tag.as_str(), name.as_ref()) {
                        // As in Chrome, geometry attributes are presentation hints of the shapes that use them.
                        ("circle" | "ellipse", "cx" | "cy")
                        | ("circle", "r")
                        | ("ellipse" | "rect", "rx" | "ry")
                        | ("rect" | "image" | "foreignObject" | "svg" | "use", "x" | "y")
                        | ("rect" | "image" | "foreignObject", "width" | "height") => {
                            Prop::find(name)
                        }
                        _ => None,
                    } {
                        // As in Chrome, a negative size is no hint, and a corner radius that is no length is 0.
                        let value = value.trim();
                        match value.parse::<f32>() {
                            _ if value.starts_with('-')
                                && matches!(prop, Prop::WIDTH | Prop::HEIGHT) => {}
                            Ok(_) => hints.push((prop, format!("{value}px").into())),
                            _ if Lengths::new(16., 100., 100.).resolve(value, 100.).is_some() => {
                                hints.push((prop, value.into()))
                            }
                            _ if matches!(prop, Prop::RX | Prop::RY) => {
                                hints.push((prop, "0px".into()))
                            }
                            _ => {}
                        }
                    } else if let Some(prop) = presentation(name) {
                        let value = value.trim();
                        let pixels = matches!(
                            prop,
                            Prop::STROKE_WIDTH
                                | Prop::STROKE_DASHOFFSET
                                | Prop::FONT_SIZE
                                | Prop::LETTER_SPACING
                                | Prop::WORD_SPACING
                        ) && value.parse::<f32>().is_ok();
                        hints.push((
                            prop,
                            if pixels {
                                format!("{value}px").into()
                            } else {
                                value.into()
                            },
                        ));
                    }
                }
                if (graphic || !hints.is_empty()) && node.tag != "svg" {
                    let mut up = node.parent;
                    while let Some(parent) = up.filter(|&parent| {
                        !matches!(doc.nodes[parent].tag.as_str(), "svg" | "foreignObject")
                    }) {
                        up = doc.nodes[parent].parent;
                    }
                    if up.is_none_or(|parent| doc.nodes[parent].tag != "svg") {
                        hints.clear();
                    } else if graphic {
                        hints.push((Prop::TRANSFORM_ORIGIN, "0 0".into()));
                    }
                }
                // As in Chrome, `width` and `height` of a video give `aspect-ratio: auto <w> / <h>`.
                if node.tag == "video"
                    && let [Some(width), Some(height)] = ["width", "height"].map(|name| {
                        let value = node.attrs.get(name)?.trim();
                        value.parse::<f32>().ok().filter(|v| *v > 0.).map(|_| value)
                    })
                {
                    hints.push((
                        Prop::ASPECT_RATIO,
                        format!("auto {width} / {height}").into(),
                    ));
                }
                // HTML table attributes, as presentation hints.
                let tag = node.tag.as_str();
                if matches!(
                    tag,
                    "table" | "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "col" | "colgroup"
                ) {
                    let length = |value: &str| -> Option<Rc<str>> {
                        let value = value.trim();
                        let (number, percent) = match value.strip_suffix('%') {
                            Some(number) => (number, true),
                            None => (value.strip_suffix("px").unwrap_or(value), false),
                        };
                        let number = number.trim().parse::<f32>().ok().filter(|v| *v >= 0.)?;
                        Some(format!("{number}{}", if percent { "%" } else { "px" }).into())
                    };
                    let cell = matches!(tag, "td" | "th");
                    let table = if cell {
                        std::iter::successors(node.parent, |&at| doc.nodes[at].parent)
                            .take(3)
                            .find(|&at| doc.nodes[at].tag == "table")
                    } else {
                        None
                    };
                    let padding = table
                        .and_then(|table| length(doc.nodes[table].attrs.get("cellpadding")?))
                        .filter(|value| !value.ends_with('%'));
                    let attrs = node
                        .attrs
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.trim()));
                    for (name, value) in
                        attrs.chain(padding.as_deref().map(|value| ("cellpadding", value)))
                    {
                        let sides = |props: [Prop; 4], value: Rc<str>| {
                            props.map(|prop| (prop, value.clone()))
                        };
                        let found: Vec<(Prop, Rc<str>)> = match (name, tag) {
                            ("width", "table" | "td" | "th" | "col" | "colgroup") => length(value)
                                .map(|value| (Prop::WIDTH, value))
                                .into_iter()
                                .collect(),
                            ("height", "table" | "td" | "th" | "tr") => length(value)
                                .map(|value| (Prop::HEIGHT, value))
                                .into_iter()
                                .collect(),
                            ("bgcolor", _) => vec![(Prop::BACKGROUND_COLOR, value.into())],
                            ("nowrap", "td" | "th") => vec![(Prop::WHITE_SPACE, "nowrap".into())],
                            ("align", "table") if value.eq_ignore_ascii_case("center") => vec![
                                (Prop::MARGIN_LEFT, "auto".into()),
                                (Prop::MARGIN_RIGHT, "auto".into()),
                            ],
                            ("align", "table") => Vec::new(),
                            ("align", _) => ["left", "right", "center", "justify"]
                                .into_iter()
                                .find(|side| value.eq_ignore_ascii_case(side))
                                .map(|side| (Prop::TEXT_ALIGN, side.into()))
                                .into_iter()
                                .collect(),
                            ("valign", _) => ["top", "middle", "bottom", "baseline"]
                                .into_iter()
                                .find(|side| value.eq_ignore_ascii_case(side))
                                .map(|side| (Prop::VERTICAL_ALIGN, side.into()))
                                .into_iter()
                                .collect(),
                            ("cellspacing", "table") => length(value)
                                .filter(|value| !value.ends_with('%'))
                                .map(|value| (Prop::BORDER_SPACING, value))
                                .into_iter()
                                .collect(),
                            ("border", "table") => sides(
                                [
                                    Prop::BORDER_TOP_WIDTH,
                                    Prop::BORDER_RIGHT_WIDTH,
                                    Prop::BORDER_BOTTOM_WIDTH,
                                    Prop::BORDER_LEFT_WIDTH,
                                ],
                                length(value)
                                    .filter(|value| !value.ends_with('%'))
                                    .unwrap_or("1px".into()),
                            )
                            .into(),
                            ("cellpadding", _) if cell => sides(
                                [
                                    Prop::PADDING_TOP,
                                    Prop::PADDING_RIGHT,
                                    Prop::PADDING_BOTTOM,
                                    Prop::PADDING_LEFT,
                                ],
                                value.into(),
                            )
                            .into(),
                            _ => Vec::new(),
                        };
                        hints.extend(found);
                    }
                }
                if node.state & (crate::dom::TOP_LAYER | crate::dom::LEAVING) != 0
                    && node.tag != "html"
                {
                    let auto = node.state & crate::dom::TOP_LAYER != 0;
                    hints.push((Prop::OVERLAY, if auto { "auto" } else { "none" }.into()));
                }
                passed = parent_style.map(|parent| {
                    if unshared {
                        return StyleRef(parent.clone());
                    }
                    let (_, passed) = style_cache
                        .heritage
                        .entry(Rc::as_ptr(parent))
                        .or_insert_with(|| {
                            let mut passed: Style = parent
                                .own()
                                .filter(|(prop, _)| prop.inherited())
                                .map(|(prop, value)| (prop, value.clone()))
                                .collect();
                            // Children of table parts take their roles from the parent's role.
                            if parent_role != 0 {
                                passed.insert(Prop::DISPLAY, TABLE_DISPLAYS[parent_role as usize]);
                            } else if item {
                                passed.insert(Prop::DISPLAY, "flex");
                            }
                            let passed = match style_cache.inherited.get(&passed) {
                                Some(old) => old.clone(),
                                None => {
                                    let passed = Rc::new(passed);
                                    style_cache.inherited.insert(passed.clone());
                                    passed
                                }
                            };
                            (parent.clone(), passed)
                        });
                    StyleRef(passed.clone())
                });
                let source = node
                    .attrs
                    .get("style")
                    .map(|source| Cow::Borrowed(source.as_str()))
                    .or_else(|| {
                        node.inline
                            .as_ref()
                            .map(|inline| Cow::Owned(inline.source()))
                    });
                let mut resolved = StyleKey {
                    scope,
                    tag: match style_cache.tags.get(node.tag.as_str()) {
                        Some(&tag) => tag,
                        None => {
                            let tag = style_cache.tags.len() as u32;
                            style_cache.tags.insert(node.tag.clone(), tag);
                            tag
                        }
                    },
                    control: node.system_control(),
                    inline: source.as_deref().map(|source| {
                        std::hash::BuildHasher::hash_one(&rustc_hash::FxBuildHasher, source)
                    }),
                    hints,
                    inputs: Inputs {
                        parent: passed.clone().map(|passed| (passed, false)),
                        ..inputs
                    },
                    rules: StyleRef(matched),
                    pseudos: matched_pseudos,
                    shadow: shadow.clone(),
                    selection: selection.cloned().map(StyleRef),
                };
                let mut values = style_cache.values.get(&resolved);
                if values.is_none()
                    && let Some(parent) = parent_style
                {
                    resolved.inputs.parent = Some((StyleRef(parent.clone()), true));
                    values = style_cache.values.get(&resolved);
                }
                let value = values.filter(|_| animated.is_none()).and_then(|values| {
                    values.iter().rev().find_map(|value| {
                        // The key doesn't hold what `style()` queries read, so the shared style must read it.
                        if value.inline.as_deref() != source.as_deref()
                            || style_reads.borrow().iter().any(|prop| {
                                value
                                    .reads
                                    .as_ref()
                                    .is_none_or(|reads| reads.iter().all(|(read, _)| read != prop))
                            })
                        {
                            return None;
                        }
                        let (vars, changed) = {
                            let held = value.vars.borrow();
                            reuse(value, held.0.as_ref(), &held.1)?
                        };
                        if changed {
                            *value.vars.borrow_mut() = (parent_vars.cloned(), vars.clone());
                        }
                        Some((value.clone(), vars))
                    })
                });
                key = Some(resolved);
                written = source;
                value
            });
        if let Some((cached, vars)) = cached {
            cached.used.set(style_cache.pass);
            if let Some(node) = scoped.nodes[id].as_mut() {
                if let Some(key) = &key {
                    node.cached = Some(CachedNode::new(
                        key,
                        parent_style,
                        cached.clone(),
                        parent_vars,
                        &vars,
                        contained.then(|| seen.clone()),
                    ));
                } else if let Some(node) = node.cached.as_mut() {
                    if let Some(parent) = parent_style {
                        node.parent = Some(StyleRef(parent.clone()));
                    }
                    if moved {
                        node.inherited = parent_vars.cloned();
                        node.vars = vars.clone();
                        *cached.vars.borrow_mut() = (parent_vars.cloned(), vars.clone());
                    }
                }
            }
            styles.set(id, cached.style.clone(), Rc::ptr_eq);
            custom.set(id, vars, Rc::ptr_eq);
            style_flags.set(id, cached.flags, u16::eq);
            lengths.set(
                id,
                Lengths {
                    viewport: [width, height],
                    ..cached.lengths
                },
            );
            if id == 0 || node.tag == "html" {
                root = cached.lengths.font;
            }
            if let Some(values) = &cached.pseudos {
                pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.text_pseudos {
                text_pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.backdrop {
                backdrops.insert(id, values.clone());
            }
            if let Some(values) = &cached.backdrop_start {
                backdrop_starts.insert(id, values.clone());
            }
            if let Some(values) = &cached.scrollbars {
                scrollbars.insert(id, values.clone());
            }
            if let Some(values) = &cached.selection {
                selections.insert(id, values.clone());
            }
            if let Some(values) = &cached.animations {
                animations.insert(id, values.clone());
            }
            if let Some(values) = &cached.important {
                important.insert(id, values.clone());
            }
            if let Some(values) = &cached.starting {
                starts.insert(id, values.clone());
            }
            for (values, list) in cached.shadowed.iter().zip([&mut *slotted, &mut *parts]) {
                if let Some(values) = values {
                    list.insert(id, values.clone());
                }
            }
            if let Some(text) = text {
                let outputs = (
                    styles[id].clone(),
                    custom[id].clone(),
                    style_flags[id],
                    lengths[id],
                    selections.get(&id).cloned(),
                );
                texts.insert(text, outputs);
            }
            settle!(id, up, seen);
            continue;
        }
        let mut key = key.unwrap();
        let mut style = match &theme {
            Some(theme) if id == 0 => (**theme).clone(),
            _ => Style::new(),
        };
        if let Some(parent) = parent_style {
            style.extend(
                parent
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone())),
            );
        }
        if let Some(vars) = parent_vars {
            style.set_vars(inherit(vars));
        }
        let mut cascade: BBTreeMap<Prop, (CascadeRank, Rc<str>)> = BBTreeMap::new();
        for &(order, specificity) in key.rules.0.iter() {
            let (_, declarations, layer) = &rules[order];
            for &(prop, ref value, important, position) in declarations.iter() {
                let rank = CascadeRank::rule(*layer, important, specificity, keys[order], position);
                put(&mut cascade, prop, rank, value.clone());
            }
        }
        for (prop, value) in &key.hints {
            let rank = CascadeRank::new(HINT, false, 0, [0; 3], 0);
            put(&mut cascade, *prop, rank, value.clone());
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (prop, value, important, position) in inline.resolved(true) {
                let rank = CascadeRank {
                    inline: true,
                    position,
                    ..CascadeRank::new(AUTHOR, important, UNLAYERED, [0; 3], u128::MAX)
                };
                put(&mut cascade, prop, rank, value);
            }
        }
        let ranked = |rule: &ShadowRule, important: bool, position: u16| CascadeRank {
            context: if important {
                -rule.context
            } else {
                rule.context
            },
            ..CascadeRank::rule(
                rule.layer,
                important,
                rule.specificity,
                rule.order,
                position,
            )
        };
        let foreign = shadow
            .iter()
            .flat_map(|rules| rules.iter())
            .filter(|rule| rule.kind == ELEMENT)
            .flat_map(|rule| {
                rule.declarations
                    .iter()
                    .map(move |(prop, value, important, position)| {
                        (*prop, value.clone(), ranked(rule, *important, *position))
                    })
            })
            .chain(
                animated
                    .into_iter()
                    .flat_map(Style::iter)
                    .map(|(prop, value)| {
                        (
                            prop,
                            value.clone(),
                            CascadeRank::new(ANIMATION, false, 0, [0; 3], 0),
                        )
                    }),
            );
        for (prop, value, rank) in foreign {
            put(&mut cascade, prop, rank, value);
        }
        if node.tag == "html"
            && let Some(scheme) = &style_cache.environment.scheme
        {
            cascade
                .entry(Prop::COLOR_SCHEME)
                .or_insert((CascadeRank::default(), scheme.clone()));
        }
        // Remove longhands reset by a stronger shorthand: longhands otherwise win during later expansion.
        for (shorthand, longhands) in [
            (
                Prop::TRANSITION,
                &[
                    Prop::TRANSITION_PROPERTY,
                    Prop::TRANSITION_DURATION,
                    Prop::TRANSITION_TIMING_FUNCTION,
                    Prop::TRANSITION_DELAY,
                    Prop::TRANSITION_BEHAVIOR,
                ][..],
            ),
            (
                Prop::ANIMATION,
                &[
                    Prop::ANIMATION_NAME,
                    Prop::ANIMATION_DURATION,
                    Prop::ANIMATION_TIMING_FUNCTION,
                    Prop::ANIMATION_DELAY,
                    Prop::ANIMATION_ITERATION_COUNT,
                    Prop::ANIMATION_DIRECTION,
                    Prop::ANIMATION_FILL_MODE,
                    Prop::ANIMATION_PLAY_STATE,
                ][..],
            ),
        ] {
            if let Some(&(rank, _)) = cascade.get(&shorthand) {
                for longhand in longhands {
                    if cascade.get(longhand).is_some_and(|(old, _)| *old < rank) {
                        cascade.remove(longhand);
                    }
                }
            }
        }
        if let Some((rank, value)) = cascade.remove(&Prop::ALL) {
            for (prop, _) in style.own() {
                cascade.entry(prop).or_insert((rank, value.clone()));
            }
            for (prop, entry) in &mut cascade {
                if entry.0 < rank
                    && !prop.is_custom()
                    && !matches!(*prop, Prop::DIRECTION | Prop::UNICODE_BIDI)
                {
                    *entry = (rank, value.clone());
                }
            }
        }
        for (&prop, (rank, value)) in cascade
            .iter_mut()
            .filter(|(_, (_, value))| matches!(&**value, "revert" | "revert-layer"))
        {
            let (revert, (origin, layer)) = (&**value == "revert", rank.source());
            let mut best: Option<(CascadeRank, &Rc<str>)> = None;
            for &(order, specificity) in key.rules.0.iter() {
                let (_, declarations, rule_layer) = &rules[order];
                let from = rule_layer.origin.min(HINT);
                if from > origin
                    || from == origin && (revert || !rank.inline && rule_layer.rank == layer)
                {
                    continue;
                }
                for (_, candidate, important, position) in declarations
                    .iter()
                    .filter(|(name, ..)| *name == prop || *name == Prop::ALL && !prop.is_custom())
                {
                    let candidate_rank = CascadeRank::rule(
                        *rule_layer,
                        *important,
                        specificity,
                        keys[order],
                        *position,
                    );
                    if !matches!(&**candidate, "revert" | "revert-layer")
                        && best.as_ref().is_none_or(|(old, _)| candidate_rank >= *old)
                    {
                        best = Some((candidate_rank, candidate));
                    }
                }
            }
            *value = best.map_or_else(|| "unset".into(), |(_, value)| value.clone());
        }
        if cascade
            .get(&Prop::OVERLAY)
            .is_some_and(|(rank, _)| !matches!(rank.level, HINT | ANIMATION))
        {
            cascade.remove(&Prop::OVERLAY);
            if let Some((_, value)) = key.hints.iter().find(|(prop, _)| *prop == Prop::OVERLAY) {
                cascade.insert(Prop::OVERLAY, (CascadeRank::default(), value.clone()));
            }
        }
        const DECORATION: [Prop; 4] = [
            Prop::TEXT_DECORATION_LINE,
            Prop::TEXT_DECORATION_STYLE,
            Prop::TEXT_DECORATION_COLOR,
            Prop::TEXT_DECORATION_THICKNESS,
        ];
        let mut decorating = false;
        if !node.tag.starts_with('#') {
            let declared = |prop: Prop| cascade.get(&prop).map(|(_, value)| &**value);
            if declared(Prop::DISPLAY).is_some_and(|value| value.starts_with("inline-"))
                || declared(Prop::FLOAT).is_some_and(|value| value != "none")
                || declared(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            {
                for prop in DECORATION {
                    style.remove(prop);
                }
            }
            match cascade
                .get(&Prop::TEXT_DECORATION_LINE)
                .map(|(_, value)| value.clone())
                .as_deref()
            {
                Some("none") if style.contains(Prop::TEXT_DECORATION_LINE) => {
                    for prop in DECORATION {
                        cascade.remove(&prop);
                    }
                }
                Some(own) if !matches!(own, "none" | "inherit" | "unset" | "initial") => {
                    decorating = true;
                    if let Some(inherited) = style
                        .get(Prop::TEXT_DECORATION_LINE)
                        .filter(|value| *value != "none")
                    {
                        let mut lines: Vec<&str> = inherited.split_ascii_whitespace().collect();
                        for line in own.split_ascii_whitespace() {
                            if !lines.contains(&line) {
                                lines.push(line);
                            }
                        }
                        let merged: Rc<str> = lines.join(" ").into();
                        if let Some((_, value)) = cascade.get_mut(&Prop::TEXT_DECORATION_LINE) {
                            *value = merged;
                        }
                    }
                }
                _ => {}
            }
        }
        physical(&mut cascade, &style);
        let explicit_size = cascade.contains_key(&Prop::FONT_SIZE);
        let mut reads = Some(style_reads.take());
        let mut missing = Vec::new();
        let mut owned = Vec::new();
        // An invalid value of a registered inheriting property takes the parent's value.
        let mut fallback = Vec::new();
        for (prop, (_, value)) in &cascade {
            if prop.is_custom() {
                if matches!(
                    &**value,
                    "inherit" | "initial" | "unset" | "revert" | "revert-layer"
                ) {
                    reads = None;
                }
                owned.push(*prop);
                if registry.get(prop).is_some_and(|entry| entry.1) {
                    fallback.push(*prop);
                }
            }
            references(value, &mut reads, &mut missing);
        }
        let agent_display = cascade
            .get(&Prop::DISPLAY)
            .is_some_and(|(rank, _)| rank.level == AGENT);
        let mut declared = Vec::new();
        let mut strong = Vec::new();
        style.extend(cascade.into_iter().filter_map(|(prop, (rank, value))| {
            if rank.important() {
                strong.push(prop);
            }
            if prop.is_custom() {
                declared.push((prop, value));
                return None;
            }
            Some((prop, value))
        }));
        resolve_style(
            &mut style,
            parent_style.map(Rc::as_ref),
            parent_vars,
            &declared,
            &registry,
            native_theme,
        );
        let parent_units = up.map(|up| lengths[up]);
        let mut keyword = resolve_keywords(
            &mut style,
            parent_style.map(Rc::as_ref),
            parent_units.as_ref(),
        );
        if decorating
            && style
                .get(Prop::TEXT_DECORATION_COLOR)
                .is_none_or(|value| value.eq_ignore_ascii_case("currentcolor"))
            && let Some(ink) = style.value(Prop::COLOR).cloned()
        {
            style.insert_current(Prop::TEXT_DECORATION_COLOR, ink);
        }
        if style.get(Prop::TEXT_ALIGN) == Some("-internal-center") {
            let inherited = parent_style
                .and_then(|parent| parent.get(Prop::TEXT_ALIGN))
                .filter(|value| !matches!(*value, "start" | "-internal-center"))
                .unwrap_or("center")
                .to_owned();
            style.insert(Prop::TEXT_ALIGN, inherited);
        }
        if style
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "-webkit-box" | "-webkit-inline-box"))
            && style
                .get(Prop::WEBKIT_BOX_ORIENT)
                .is_some_and(|value| value == "vertical")
            && style
                .get(Prop::WEBKIT_LINE_CLAMP)
                .is_some_and(|value| value != "none")
        {
            let display = if style.get(Prop::DISPLAY) == Some("-webkit-box") {
                "flow-root"
            } else {
                "inline-block"
            };
            style.insert(Prop::DISPLAY, display);
        }
        // AppKit draws the control, so the box has no padding, border or background.
        if node.native_control(&style).is_some() {
            for prop in crate::css::PADDING
                .into_iter()
                .chain(
                    crate::css::BORDERS
                        .into_iter()
                        .flat_map(|side| [side[0], side[1]]),
                )
                .chain([Prop::BACKGROUND_COLOR, Prop::BACKGROUND_IMAGE])
            {
                style.remove(prop);
            }
        }
        let mut role = 0;
        if !node.tag.starts_with('#') {
            let root = node.parent == Some(0);
            if root && style.get(Prop::DISPLAY) == Some("contents") {
                style.insert(Prop::DISPLAY, "block");
            }
            blockify(&mut style, item || root, table_item);
            let grouped = table_box(parent_role) || matches!(parent_role, 5..=7);
            let display;
            (role, display) = match style.get(Prop::DISPLAY).unwrap_or("inline") {
                "table" => (1, "grid"),
                "inline-table" => (2, "inline-grid"),
                // A `<table>` box holds the anonymous table of its rows.
                "block" if node.tag == "table" => (3, "grid"),
                "table-row" if grouped => (ROW, "contents"),
                "table-row-group" if table_box(parent_role) => (5, "contents"),
                "table-header-group" if table_box(parent_role) => (HEADER, "contents"),
                "table-footer-group" if table_box(parent_role) => (FOOTER, "contents"),
                "table-row" => (10, "grid"),
                "table-row-group" | "table-footer-group" => (11, "grid"),
                "table-header-group" => (12, "grid"),
                "table-cell" => (CELL, "block"),
                "table-caption" => (CAPTION, "block"),
                _ => (0, ""),
            };
            if role != 0 {
                style.insert(Prop::DISPLAY, display);
            }
            // Like a table, the grid is as wide as its content when its width is `auto`.
            if table_box(role) && style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                style.insert(Prop::WIDTH, "fit-content");
            }
        }

        if table_box(role) {
            border_spacing(&mut style);
        }
        let parent_weight = parent_style
            .and_then(|parent| parent.get(Prop::FONT_WEIGHT))
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(400.);
        if let Some(weight) = style.get(Prop::FONT_WEIGHT) {
            let value = match weight {
                "normal" => 400.,
                "bold" => 700.,
                "bolder" if parent_weight < 350. => 400.,
                "bolder" if parent_weight < 550. => 700.,
                "bolder" => 900f32.max(parent_weight),
                "lighter" if parent_weight < 550. => 100f32.min(parent_weight),
                "lighter" if parent_weight < 750. => 400.,
                "lighter" => 700.,
                value => value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| (1. ..=1000.).contains(v))
                    .unwrap_or(parent_weight),
            };
            style.insert(Prop::FONT_WEIGHT, value.to_string());
        }
        if let Some(family) = context.and_then(|context| {
            context
                .aliases
                .get(style.get(Prop::FONT_FAMILY)?.trim_matches(['\'', '"']))
        }) {
            style.insert(Prop::FONT_FAMILY, family.as_str());
        }
        let mut units = Lengths {
            font: parent,
            root,
            viewport: [width, height],
            container: None,
        };
        let queried = [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains("cq")));
        if queried
            && let Some(container) =
                std::iter::successors(seen.as_deref(), |link| link.up.as_deref())
                    .find(|link| link.sized)
        {
            style_cache.sized.insert(container.id);
            units.container = style_cache.containers.get(&container.id).copied();
        }
        let viewport = queried && units.container.is_none()
            || [Prop::FONT_SIZE, Prop::LINE_HEIGHT]
                .into_iter()
                .any(|prop| {
                    style.get(prop).is_some_and(|value| {
                        ["vw", "vh", "vi", "vb", "vmin", "vmax"]
                            .iter()
                            .any(|unit| value.contains(unit))
                    })
                });
        let size = match style.get(Prop::FONT_SIZE).unwrap_or("medium") {
            "xx-small" => Some(9.),
            "x-small" => Some(10.),
            "small" => Some(13.),
            "medium" => Some(16.),
            "large" => Some(18.),
            "x-large" => Some(24.),
            "xx-large" => Some(32.),
            "xxx-large" => Some(48.),
            "larger" => Some(parent.em * 1.2),
            "smaller" => Some(parent.em / 1.2),
            value => units.resolve(value, parent.em),
        }
        .filter(|size| size.is_finite() && *size >= 0.)
        // Glyph rasterization allocates by font size, so huge sizes abort the process.
        .map(|size| size.min(10_000.))
        .unwrap_or(parent.em);
        let monospace = |style: Option<&Style>| {
            style
                .and_then(|style| style.get(Prop::FONT_FAMILY))
                .is_some_and(|family| family.trim().eq_ignore_ascii_case("monospace"))
        };
        let absolute = match style.get(Prop::FONT_SIZE).filter(|_| explicit_size) {
            None => parent.absolute,
            Some(value)
                if value.starts_with("calc(")
                    || matches!(value, "larger" | "smaller")
                    || ["%", "em", "ex", "ch", "cap", "ic", "lh"]
                        .iter()
                        .any(|unit| value.ends_with(unit)) =>
            {
                parent.absolute
            }
            Some(value) => value.starts_with(|c: char| c.is_ascii_digit() || c == '.'),
        };
        let size = if !explicit_size
            && !parent.absolute
            && monospace(Some(&style))
            && !monospace(parent_style.map(Rc::as_ref))
        {
            size * 13. / 16.
        } else {
            size
        };
        units.font = FontUnits::new(size);
        units.font.absolute = absolute;
        if let Some(text) = text_system {
            let font = font(&style);
            let (id, metrics) = font_metrics.entry(font.clone()).or_insert_with(|| {
                let id = text.face(&font);
                let size = renderer::px(1.);
                (
                    id,
                    [
                        text.advance(id, size, '0').map(f32::from).unwrap_or(0.5),
                        text.metrics(id).x_height * size.0,
                        text.metrics(id).cap_height * size.0,
                        text.advance(id, size, '水').map(f32::from).unwrap_or(1.),
                    ],
                )
            });
            let [ch, ex, cap, ic] = metrics.map(|value| value * size);
            units.font = FontUnits {
                em: size,
                absolute,
                ch,
                ex,
                cap,
                ic,
                lh: text
                    .line_metrics(*id, renderer::px(size * scale))
                    .into_iter()
                    .map(f32::from)
                    .sum::<f32>()
                    / scale,
            };
        }
        let ratio = ((native_theme >> 16) as f32).max(100.) / 100.;
        if let Some(text) = style.get(Prop::LINE_HEIGHT) {
            let mut line_units = units;
            line_units.font.lh = parent.lh;
            if node.tag == "html" {
                line_units.root = units.font;
                line_units.root.lh = parent.lh;
            }
            if let Some((value, dimensional)) = line_units.parse(text, size)
                && value >= 0.
            {
                units.font.lh =
                    if dimensional { value } else { value * size }.min(33_554_431. / ratio);
                style.insert(
                    Prop::LINE_HEIGHT,
                    // Like Chrome, whose layout units count device pixels, a length is at most 2^25 device
                    // pixels, and a number gives at most the f32 range in device pixels.
                    if dimensional {
                        format!("{}px", value.min(33_554_432. / ratio))
                    } else if value >= 33_554_428. {
                        units
                            .math_unclamped(text, f32::NAN, &[])
                            .map_or(value, |(number, _)| {
                                (number as f32).min(f32::MAX / ratio / size)
                            })
                            .to_string()
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        // Like Chrome, border and outline widths snap to device pixels, and a width or offset under one device
        // pixel is one device pixel.
        for prop in [
            Prop::BORDER_TOP_WIDTH,
            Prop::BORDER_RIGHT_WIDTH,
            Prop::BORDER_BOTTOM_WIDTH,
            Prop::BORDER_LEFT_WIDTH,
            Prop::OUTLINE_WIDTH,
            Prop::COLUMN_RULE_WIDTH,
            Prop::OUTLINE_OFFSET,
        ] {
            let Some(value) = style.get(prop) else {
                continue;
            };
            // Keywords and whole device pixels, the common case, stay without parsing.
            let whole = |number: &str| {
                number
                    .parse::<f32>()
                    .is_ok_and(|number| (number * ratio).fract() == 0.)
            };
            if matches!(value, "0" | "thin" | "medium" | "thick")
                || value.strip_suffix("px").is_some_and(whole)
            {
                continue;
            }
            let Some(width) = units.resolve(value, f32::NAN) else {
                continue;
            };
            let device = width * ratio;
            let snapped = match device {
                _ if device != 0. && device.abs() < 1. => device.signum(),
                _ => device.trunc(),
            } / ratio;
            if snapped != width {
                style.insert(prop, format!("{}px", number(f64::from(snapped))));
            }
        }
        if id == 0 || node.tag == "html" {
            root = units.font;
        }
        units.root = root;
        style.insert(Prop::FONT_SIZE, format!("{size}px"));
        // Like Chrome, these keywords are 1, 3 and 5 24ths of the font size.
        if let Some(index) = (style.get(Prop::WEBKIT_TEXT_STROKE_WIDTH)).and_then(|value| {
            ["thin", "medium", "thick"]
                .iter()
                .position(|name| *name == value)
        }) {
            let width = units.font.em * (index * 2 + 1) as f32 / 24.;
            style.insert(Prop::WEBKIT_TEXT_STROKE_WIDTH, format!("{width}px"));
        }
        // Children inherit these lengths, not their units.
        for prop in [
            Prop::LETTER_SPACING,
            Prop::WORD_SPACING,
            Prop::TEXT_INDENT,
            Prop::TEXT_UNDERLINE_OFFSET,
            Prop::WEBKIT_TEXT_STROKE_WIDTH,
            Prop::TEXT_SHADOW,
        ] {
            if let Some(value) = style.get(prop).and_then(|value| pixels(value, &units)) {
                style.insert(prop, value);
            }
        }
        let mut pseudo_cascade: [BBTreeMap<Prop, (CascadeRank, &Rc<str>)>; 17] = Default::default();
        let mut groups: [Vec<Passed>; 2] = Default::default();
        let own = Container::of(
            id,
            &style,
            named_queries.then(|| style.vars()),
            seen.clone(),
        );
        let mut deferred = false;
        let mut query = |conditions: &ContainerQuery, key: (usize, usize)| {
            deferred = true;
            query_containers(
                conditions,
                own.as_ref().or(seen.as_deref()),
                key,
                &style_cache.containers,
                |container| {
                    if container == id {
                        units
                    } else {
                        lengths[container]
                    }
                },
                &mut style_cache.queries,
                (Some(style.vars()), &style_reads),
            )
        };
        // Pseudo-elements query their own element, so their conditional rules are checked again here.
        let again: Option<Vec<usize>> = (own.is_some()
            || scoped.nodes[id].iter().any(|matched| {
                matched
                    .pseudos
                    .iter()
                    .any(|&index| scoped.containers[scoped.pseudos[index].2].is_some())
            }))
        .then(|| {
            scoped.nodes[id]
                .iter()
                .flat_map(|matched| matched.pseudos.iter().copied())
                .filter(|&index| {
                    let (kind, _, order, _) = &scoped.pseudos[index];
                    match &scoped.containers[*order] {
                        Some(conditions) if !matches!(kind, SLOTTED..=STARTING) => {
                            !scoped.medias[*order]
                                .as_ref()
                                .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
                                && query(conditions, (scope, *order))
                        }
                        _ => key.pseudos.binary_search(&index).is_ok(),
                    }
                })
                .collect()
        });
        if let Some(reads) = &mut reads {
            reads.extend(style_reads.take());
        }
        for &index in again.as_deref().unwrap_or(&key.pseudos) {
            let (pseudo, selector, order, _) = &scoped.pseudos[index];
            let (_, declarations, layer) = &rules[*order];
            if matches!(*pseudo, SLOTTED | PART) {
                if let Some((name, compound, target)) = scoped.arguments.get(&index) {
                    groups[usize::from(*pseudo == PART)].push(Passed {
                        name: name.clone(),
                        compound: compound.clone(),
                        conditions: scoped.containers[*order]
                            .clone()
                            .map(|conditions| (scope, *order, conditions)),
                        rule: ShadowRule {
                            kind: *target,
                            context: if *pseudo == PART { 1 } else { -1 },
                            layer: *layer,
                            specificity: selector.specificity,
                            order: keys[*order],
                            declarations: declarations.clone(),
                            query: None,
                        },
                    });
                }
                continue;
            }
            for &(prop, ref value, important, position) in declarations.iter() {
                let rank = CascadeRank::rule(
                    *layer,
                    important,
                    selector.specificity,
                    keys[*order],
                    position,
                );
                put(&mut pseudo_cascade[*pseudo], prop, rank, value);
            }
        }
        for rule in shadow
            .iter()
            .flat_map(|rules| rules.iter())
            .filter(|rule| rule.kind != ELEMENT)
        {
            if let Some((scope, order, applies)) = rule.query
                && !match own {
                    Some(_) => waiting
                        .iter()
                        .find(|waits| (waits.0, waits.1) == (scope, order))
                        .is_some_and(|waits| query(&waits.2, (scope, order))),
                    None => applies,
                }
            {
                continue;
            }
            for (prop, value, important, position) in rule.declarations.iter() {
                let rank = ranked(rule, *important, *position);
                put(&mut pseudo_cascade[rule.kind], *prop, rank, value);
            }
        }
        for (prop, (_, value)) in pseudo_cascade.iter().flatten() {
            if prop.is_custom() {
                reads = None;
            }
            references(value, &mut reads, &mut missing);
        }
        let keep = [
            Prop::ANIMATION,
            Prop::ANIMATION_NAME,
            Prop::TRANSITION,
            Prop::TRANSITION_PROPERTY,
        ]
        .into_iter()
        .any(|prop| {
            style.contains(prop)
                || pseudo_cascade[..2]
                    .iter()
                    .any(|cascade| cascade.contains_key(&prop))
        });
        let [
            before,
            after,
            selected,
            marker,
            letter,
            line,
            placeholder,
            bar,
            track,
            thumb,
            corner,
            mut hovered,
            _,
            _,
            starting,
            backdrop,
            backdrop_starting,
        ] = pseudo_cascade;
        let starting = (!starting.is_empty()).then(|| {
            let mut values = style.clone();
            let mut declared = Vec::new();
            values.extend(starting.into_iter().filter_map(|(prop, (_, value))| {
                if prop.is_custom() {
                    declared.push((prop, value.clone()));
                    return None;
                }
                Some((prop, value.clone()))
            }));
            resolve_style(
                &mut values,
                parent_style.map(Rc::as_ref),
                parent_vars,
                &declared,
                &registry,
                native_theme,
            );
            keyword |= resolve_keywords(
                &mut values,
                parent_style.map(Rc::as_ref),
                parent_units.as_ref(),
            );
            if !keep && declared.is_empty() {
                values.set_vars(empty.clone());
            }
            let values = Rc::new(values);
            starts.insert(id, values.clone());
            values
        });
        // A pseudo-element inherits custom properties from its element, like a child.
        let handed = inherit(style.vars());
        let derive = |mut cascade: BBTreeMap<Prop, (CascadeRank, &Rc<str>)>| {
            (!cascade.is_empty()).then(|| {
                physical(&mut cascade, &style);
                let mut values: Style = style
                    .own()
                    .filter(|(prop, _)| prop.inherited())
                    .map(|(prop, value)| (prop, value.clone()))
                    .collect();
                values.set_vars(handed.clone());
                let mut declared = Vec::new();
                values.extend(cascade.into_iter().filter_map(|(prop, (_, value))| {
                    if prop.is_custom() {
                        declared.push((prop, value.clone()));
                        return None;
                    }
                    Some((prop, value.clone()))
                }));
                resolve_style(
                    &mut values,
                    Some(&style),
                    None,
                    &declared,
                    &registry,
                    native_theme,
                );
                resolve_keywords(&mut values, Some(&style), Some(&units));
                if !keep && declared.is_empty() {
                    values.set_vars(empty.clone());
                }
                values
            })
        };
        let layered =
            node.state & (crate::dom::TOP_LAYER | crate::dom::LEAVING) != 0 && node.tag != "html";
        if layered && !backdrop_starting.is_empty() {
            let mut first = backdrop.clone();
            for (prop, (rank, value)) in backdrop_starting {
                put(&mut first, prop, rank, value);
            }
            if let Some(values) = derive(first) {
                backdrop_starts.insert(id, Rc::new(values));
            }
        }
        if layered && let Some(values) = derive(backdrop) {
            backdrops.insert(id, Rc::new(values));
        }
        if [&marker, &letter, &line, &placeholder]
            .iter()
            .any(|cascade| !cascade.is_empty())
        {
            text_pseudos.insert(
                id,
                Rc::new([marker, letter, line, placeholder].map(&derive)),
            );
        }
        if [&bar, &track, &thumb, &corner, &hovered]
            .iter()
            .any(|cascade| !cascade.is_empty())
            && (matches!(node.tag.as_str(), "html" | "body" | "progress")
                || node.tag == "input"
                    && ["range", "file"]
                        .iter()
                        .any(|kind| node.input_type().eq_ignore_ascii_case(kind))
                || [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| {
                        style
                            .get(prop)
                            .is_some_and(|value| matches!(value, "auto" | "scroll"))
                    }))
        {
            if !hovered.is_empty() {
                for (prop, value) in &thumb {
                    hovered.entry(*prop).or_insert(*value);
                }
            }
            scrollbars.insert(
                id,
                Rc::new([bar, track, thumb, corner, hovered].map(&derive)),
            );
        }
        if !selected.is_empty() || up.is_some_and(|up| selections.contains_key(&up)) {
            let parent = up.and_then(|up| selections.get(&up));
            let mut values = parent.map(|parent| (**parent).clone()).unwrap_or_default();
            values.set_vars(handed.clone());
            if let Some(scheme) = style.value(Prop::COLOR_SCHEME) {
                values.insert(Prop::COLOR_SCHEME, scheme.clone());
            }
            let fallback = |prop: Prop| -> Rc<str> {
                parent
                    .and_then(|parent| parent.value(prop))
                    .cloned()
                    .unwrap_or_else(|| {
                        if prop == Prop::BACKGROUND_COLOR {
                            "transparent"
                        } else {
                            "currentColor"
                        }
                        .into()
                    })
            };
            let painted = |prop: Prop| {
                matches!(
                    prop,
                    Prop::COLOR | Prop::BACKGROUND_COLOR | Prop::WEBKIT_TEXT_FILL_COLOR
                )
            };
            for (prop, (_, value)) in selected {
                if painted(prop) {
                    values.insert(
                        prop,
                        if matches!(&**value, "inherit" | "unset") {
                            fallback(prop)
                        } else {
                            value.clone()
                        },
                    );
                }
            }
            let colored = values.contains(Prop::COLOR);
            resolve_style(
                &mut values,
                parent.map(|parent| &**parent),
                None,
                &[],
                &registry,
                native_theme,
            );
            if !keep {
                values.set_vars(empty.clone());
            }
            if !colored {
                values.remove(Prop::COLOR);
            }
            values.retain(|prop, value| {
                if !painted(prop) {
                    return false;
                }
                if matches!(&**value, "inherit" | "unset") {
                    *value = fallback(prop);
                } else if &**value == "initial" {
                    *value = if prop == Prop::BACKGROUND_COLOR {
                        "transparent"
                    } else {
                        "currentColor"
                    }
                    .into();
                }
                if value.eq_ignore_ascii_case("currentcolor") || color(value).is_some() {
                    true
                } else if let Some(inherited) = parent.and_then(|parent| parent.value(prop)) {
                    *value = inherited.clone();
                    true
                } else {
                    false
                }
            });
            if !values.is_empty() {
                let values = match parent {
                    Some(parent) if **parent == values => parent.clone(),
                    _ => Rc::new(values),
                };
                selections.insert(id, values);
            }
        }
        if [&before, &after]
            .iter()
            .any(|values| values.contains_key(&Prop::CONTENT))
        {
            pseudos.insert(
                id,
                Rc::new([before, after].map(|cascade| {
                    let mut values = derive(cascade).unwrap_or_default();
                    if values.contains(Prop::CONTENT) {
                        let display = style.get(Prop::DISPLAY);
                        let flex = matches!(
                            display,
                            Some("flex" | "inline-flex" | "grid" | "inline-grid")
                        );
                        blockify(
                            &mut values,
                            flex || item && display == Some("contents"),
                            false,
                        );
                    }
                    values
                })),
            );
        }
        if node.tag != "slot" {
            groups[0].clear();
        }
        let [inner, outer] = groups;
        let mut store = |list: Vec<Passed>, part: bool| {
            (!list.is_empty()).then(|| {
                let list = last
                    .and_then(|last| if part { &last.parts } else { &last.slotted }.get(&id))
                    .filter(|old| ***old == list)
                    .cloned()
                    .unwrap_or_else(|| Rc::new(list));
                if part { &mut *parts } else { &mut *slotted }.insert(id, list.clone());
                list
            })
        };
        let shadowed = [store(inner, false), store(outer, true)];
        let kept = previous.filter(|_| animated.is_some());
        if let Some(previous) = kept {
            if let Some(effects) = previous.animations.get(&id) {
                animations.insert(id, effects.clone());
            }
        } else {
            let mut effects =
                if style.contains(Prop::ANIMATION) || style.contains(Prop::ANIMATION_NAME) {
                    Animation::parse(&style, &scoped.keyframes, &registry, native_theme)
                } else {
                    Vec::new()
                };
            for (slot, pseudo) in pseudos
                .get(&id)
                .into_iter()
                .flat_map(|pair| pair.iter().enumerate())
            {
                if pseudo.contains(Prop::ANIMATION) || pseudo.contains(Prop::ANIMATION_NAME) {
                    effects.extend(
                        Animation::parse(pseudo, &scoped.keyframes, &registry, native_theme)
                            .into_iter()
                            .map(|mut effect| {
                                effect.pseudo = slot as u8 + 1;
                                effect.name =
                                    format!("{}\0{}", ["::before", "::after"][slot], effect.name);
                                effect
                            }),
                    );
                }
            }
            if !effects.is_empty() {
                animations.insert(id, effects.into());
            }
        }
        // Reuse equal styles from the previous frame. Pointer changes invalidate layout caches.
        let units_bits = |units: &Lengths| [units.font.bits(), units.root.bits()];
        let vars = match context
            .and_then(|context| context.previous.as_deref())
            .and_then(|previous| previous.vars.get(id))
        {
            Some(old) if !Rc::ptr_eq(old, style.vars()) && **old == **style.vars() => old.clone(),
            _ => style.vars().clone(),
        };
        let reads = reads.filter(|_| !keep).map(|mut names| {
            names.sort_unstable();
            names.dedup();
            names
                .into_iter()
                .filter(|prop| !owned.contains(prop))
                .chain(fallback)
                .map(|prop| (prop, parent_vars.and_then(|vars| var(vars, prop)).cloned()))
                .collect()
        });
        let own = owned
            .into_iter()
            .map(|prop| (prop, var(&vars, prop).cloned()))
            .collect();
        style.set_vars(if keep { vars.clone() } else { empty.clone() });
        let get = |prop| style.get(prop).unwrap_or("");
        let flag = |set, value| if set { value } else { 0 };
        let counted = [
            Prop::COUNTER_RESET,
            Prop::COUNTER_INCREMENT,
            Prop::COUNTER_SET,
        ]
        .into_iter()
        .any(|prop| style.contains(prop))
            || pseudos.get(&id).is_some_and(|pseudos| {
                pseudos.iter().any(|pseudo| {
                    pseudo
                        .get(Prop::CONTENT)
                        .is_some_and(|content| content.contains("counter"))
                        || pseudo.contains(Prop::COUNTER_INCREMENT)
                        || pseudo.contains(Prop::COUNTER_RESET)
                })
            });
        let bits = flag(get(Prop::DISPLAY) == "none", DISPLAY_NONE)
            | flag(get(Prop::DISPLAY) == "contents", DISPLAY_CONTENTS)
            | flag(
                matches!(get(Prop::VISIBILITY), "hidden" | "collapse"),
                INVISIBLE,
            )
            | match get(Prop::USER_SELECT) {
                "none" => SELECT_NONE,
                "all" => SELECT_ALL,
                "text" => SELECT_TEXT,
                _ => 0,
            }
            | flag(counted, COUNTED)
            | flag(agent_display, AGENT_DISPLAY)
            | role << 8;
        style_flags.set(id, bits, u16::eq);
        let style = match context
            .and_then(|context| context.previous.as_deref())
            .and_then(|previous| {
                Some((
                    previous,
                    previous.authored.get(&id).or(previous.values.get(id))?,
                ))
            }) {
            Some((previous, old))
                if **old == style
                    && previous.anonymous.get(&id).or(previous.flags.get(id))
                        == Some(&style_flags[id])
                    && previous.lengths.get(id).is_some_and(|old| {
                        old.container.is_none()
                            && units.container.is_none()
                            && units_bits(old) == units_bits(&units)
                    })
                    && previous.pseudos.get(&id) == pseudos.get(&id)
                    && previous.text_pseudos.get(&id) == text_pseudos.get(&id)
                    && previous.backdrops.get(&id) == backdrops.get(&id)
                    && previous.backdrop_starts.get(&id) == backdrop_starts.get(&id)
                    && previous.scrollbars.get(&id) == scrollbars.get(&id)
                    && previous.selections.get(&id) == selections.get(&id)
                    && previous.starting.get(&id) == starting.as_ref()
                    && !previous.animations.contains_key(&id)
                    && !animations.contains_key(&id)
                    // A `contents` box needs a new style when its ancestors change the items below it.
                    && style_flags[id] & DISPLAY_CONTENTS == 0 =>
            {
                old.clone()
            }
            _ => Rc::new(style),
        };
        let cached = Rc::new(CachedStyle {
            used: std::cell::Cell::new(style_cache.pass),
            inline: written.map(Into::into),
            style: style.clone(),
            vars: std::cell::RefCell::new((parent_vars.cloned(), vars.clone())),
            reads,
            own,
            missing: missing.into_boxed_slice(),
            flags: style_flags[id],
            lengths: units,
            pseudos: pseudos.get(&id).cloned(),
            text_pseudos: text_pseudos.get(&id).cloned(),
            backdrop: backdrops.get(&id).cloned(),
            backdrop_start: backdrop_starts.get(&id).cloned(),
            scrollbars: scrollbars.get(&id).cloned(),
            selection: selections.get(&id).cloned(),
            animations: animations.get(&id).cloned(),
            important: (!strong.is_empty()).then(|| {
                let values: Rc<[Prop]> = strong.into();
                important.insert(id, values.clone());
                values
            }),
            shadowed,
            starting,
            viewport,
            keyword,
        });
        if animated.is_none() {
            if let Some(node) = scoped.nodes[id].as_mut() {
                node.cached = Some(CachedNode::new(
                    &key,
                    parent_style,
                    cached.clone(),
                    parent_vars,
                    &vars,
                    (contained || queried || deferred).then(|| seen.clone()),
                ));
            }
            // The key doesn't hold the container, so styles with container units are not shared.
            if !queried && !deferred {
                if !keyword && passed.is_some() {
                    key.inputs.parent = passed.map(|passed| (passed, false));
                }
                let values = style_cache.values.entry(key).or_default();
                if values.len() == 8 {
                    values.remove(0);
                }
                values.push(cached);
            }
        }
        styles.set(id, style, Rc::ptr_eq);
        custom.set(id, vars, Rc::ptr_eq);
        lengths.set(id, units);
        settle!(id, up, seen);
    }
    style_cache.inheritable = inheritable;
    style_cache.changed = changed;
    if stored {
        if !fast {
            matches.dirty.fill(false);
        }
        style_cache.full = false;
        style_cache.styled = Some(Styled {
            nodes: walk.walk.nodes.clone(),
            serial: walk.walk.serial,
            tree: walk.tree.clone(),
            trees: walk.trees.clone(),
            environment,
            scopes: used,
        });
    }
    number_lists(
        doc, &walk, &mut out, previous, last, fast, recount, counters,
    );
    style_cache.collapse_tables(doc, &walk, &mut out, [width, height], previous, fast);
    anonymous_tables(doc, &walk, &shadows, &mut out, previous, fast, &mut orphans);
    style_cache.orphans = orphans;
    #[cfg(debug_assertions)]
    if fast {
        check_incremental(
            doc,
            width,
            height,
            text_system,
            cache,
            style_cache,
            context,
            &out,
        );
    }
    out
}

// Checks that an incremental pass gives what a full pass gives.
#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
fn check_incremental(
    doc: &Document,
    width: f32,
    height: f32,
    text_system: Option<&renderer::TextSystem>,
    cache: &mut HashMap<Rc<str>, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
    out: &ComputedStyles,
) {
    style_cache.incremental += 1;
    let mut fresh = StyleCache {
        cssom: style_cache.cssom.clone(),
        constructed: style_cache.constructed.clone(),
        adopted: style_cache.adopted.clone(),
        containers: style_cache.containers.clone(),
        scripted: style_cache.scripted.clone(),
        tailwind: std::mem::take(&mut style_cache.tailwind),
        unshared: true,
        ..Default::default()
    };
    let full = compute_styles(doc, width, height, text_system, cache, &mut fresh, context);
    style_cache.tailwind = fresh.tailwind;
    let groups = |a: Option<&Shadowed>, b: Option<&Shadowed>| match (a, b) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter().zip(b.iter()).all(|(a, b)| {
                    a.name == b.name
                        && a.compound.len() == b.compound.len()
                        && a.conditions.is_some() == b.conditions.is_some()
                        && (a.rule.kind, a.rule.context, a.rule.specificity)
                            == (b.rule.kind, b.rule.context, b.rule.specificity)
                        && a.rule.declarations == b.rule.declarations
                })
        }
        (a, b) => a.is_none() && b.is_none(),
    };
    for id in 0..doc.nodes.len() {
        let same = [
            groups(full.slotted.get(&id), out.slotted.get(&id)),
            groups(full.parts.get(&id), out.parts.get(&id)),
            groups(full.exported.get(&id), out.exported.get(&id)),
            *full.values[id] == *out.values[id],
            full.vars[id] == out.vars[id],
            full.flags[id] == out.flags[id],
            full.lengths[id] == out.lengths[id],
            full.pseudos.get(&id) == out.pseudos.get(&id),
            full.text_pseudos.get(&id) == out.text_pseudos.get(&id),
            full.backdrops.get(&id) == out.backdrops.get(&id),
            full.backdrop_starts.get(&id) == out.backdrop_starts.get(&id),
            full.scrollbars.get(&id) == out.scrollbars.get(&id),
            full.selections.get(&id) == out.selections.get(&id),
            format!("{:?}", full.animations.get(&id)) == format!("{:?}", out.animations.get(&id)),
            full.important.get(&id) == out.important.get(&id),
            full.starting.get(&id) == out.starting.get(&id),
            full.containers[id] == out.containers[id],
        ];
        assert!(
            same.iter().all(|same| *same),
            "incremental style of node {id} <{}> differs from a full pass: {same:?}",
            doc.nodes[id].tag
        );
    }
}
