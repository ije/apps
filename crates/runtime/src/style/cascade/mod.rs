use crate::css::{
    properties::Prop, properties::Registry, properties::Style, properties::Vars,
    properties::physical, sheet::Stylesheet, value::units::Lengths,
};
use crate::dom::{Document, SelectorCache};
use crate::style::cascade::computed::{ComputedStyles, empty};
use crate::style::cascade::container::QueryCache;
use crate::style::cascade::memo::{CachedStyle, Inheritable, StyleKey};
use crate::style::cascade::node::NodePass;
use crate::style::cascade::scope::{Collect, MediaBlock, MediaStack, StyleScope};
#[cfg(debug_assertions)]
use crate::style::cascade::shadow::Shadowed;
use crate::style::cascade::sheets::{Cssom, Derived, Sheet};
use crate::style::cascade::table::CollapsedTable;
use crate::style::fonts::font_names;
use crate::style::{used::resolve_keywords, vars::resolve_style};
use counters::number_lists;
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::rc::Rc;
use table::anonymous_tables;

pub(crate) mod computed;
mod container;
mod counters;
mod declared;
mod highlight;
mod hints;
mod memo;
mod moves;
mod node;
pub(super) mod rank;
pub(super) mod scope;
mod shadow;
mod sheets;
mod table;
pub(crate) use table::{Slots, span};
mod used_values;

pub(crate) use computed::Used;
pub(crate) use container::Container;
pub(crate) use highlight::Highlights;
pub(crate) use shadow::Passed;

#[derive(Default)]
pub(crate) struct StyleCache {
    pub(crate) environment: Environment,
    pub(crate) scopes: Vec<StyleScope>,
    pub(crate) next: usize,
    pub(crate) pass: u64,
    pub(crate) states: u32,
    pub(crate) sheets: HashMap<usize, Sheet>,
    // The `media` attribute of each `<style>` and `<link>`: the pass that used it, its text and its query.
    pub(crate) media: HashMap<usize, (u64, String, Stylesheet)>,
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
    // A style had anchor positioning, the results of the last layout, and the styles made with them, from the own
    // ones.
    pub(crate) anchoring: bool,
    pub(crate) anchored: Rc<FxHashMap<usize, Rc<Used>>>,
    #[allow(clippy::type_complexity)]
    pub(crate) anchored_styles: FxHashMap<usize, (Rc<Style>, Rc<Used>, Rc<Style>)>,
    // Counts the changes of the `@position-try` rules.
    pub(crate) rules: u64,
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
    pub(crate) native_theme: u32,
    pub(crate) registry: Rc<Registry>,
    pub(crate) scheme: Option<Rc<str>>,
    // The Tailwind theme, whose variables the document node has.
    pub(crate) theme: Option<Rc<Style>>,
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
            // A patched walk keeps the lists when no node in them, no node for them and no shadow root moved or
            // changed. Nodes that moved keep the tree of their parent, which comes before them in the walk.
            let patched = walk.walk.since(serial).filter(|touched| {
                touched.iter().all(|&id| {
                    let id = id as usize;
                    walk.listed.binary_search(&id).is_err()
                        && !(walk.walk.contains(id) && special(id))
                        && (shadows.hosts.is_empty() || !shadows.hosts.contains_key(&id))
                })
            });
            // Without shadow trees, every node is in the first tree, so `tree` stays as it is.
            if let Some(touched) = patched.filter(|_| walk.trees.len() > 1) {
                let tree = Rc::make_mut(&mut walk.tree);
                tree.resize(doc.nodes.len(), u32::MAX);
                let mut joined: Vec<(u32, u32)> = touched
                    .iter()
                    .filter_map(|&id| {
                        let at = walk.walk.place(id as usize);
                        if at.is_none() {
                            tree[id as usize] = u32::MAX;
                        }
                        Some((at?, id))
                    })
                    .collect();
                joined.sort_unstable();
                let last = walk.trees.len() as u32 - 1;
                for (_, id) in joined {
                    tree[id as usize] = doc.nodes[id as usize]
                        .parent
                        .map_or(0, |parent| tree[parent].min(last));
                }
            }
            if patched.is_none() || cfg!(debug_assertions) {
                let mut tree = vec![u32::MAX; doc.nodes.len()];
                let mut trees = vec![(0, usize::MAX)];
                let mut lists = vec![Vec::new()];
                let mut tables = Vec::new();
                for &(id, _) in walk.walk.order().iter() {
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
                    patched.is_none()
                        || (&trees, &lists, &tables) == (&*walk.trees, &walk.special, &walk.tables)
                            && (trees.len() == 1 || tree == *walk.tree),
                    "patched style walk lists differ"
                );
                if patched.is_none() {
                    let mut listed: Vec<usize> =
                        lists.iter().flatten().chain(&tables).copied().collect();
                    listed.sort_unstable();
                    walk.listed = listed;
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
        self.media.retain(|_, (used, ..)| *used == pass);
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
    // The nodes of `special` and `tables`, sorted.
    pub(crate) listed: Vec<usize>,
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
    let native_theme = context.map_or(0, |context| context.theme);
    if let Some(text) = text_system {
        font_names(
            text,
            context.map_or(&HashMap::new(), |context| &context.aliases),
        );
    }
    let blank = || Collect::new(width, height, native_theme);
    let shadows = context.map_or_else(Default::default, |context| context.shadows.clone());
    style_cache.shadows = shadows.clone();
    let walk = style_cache.walk(doc, &shadows);
    let order = walk.walk.order();
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
    let scale = context.map_or(1., |context| context.scale);
    let metrics = text_system.map(|_| scale);
    let old = &mut style_cache.environment;
    if old.viewport != [width, height] || old.metrics != metrics || old.native_theme != native_theme
    {
        fast = false;
        style_cache.full = true;
        let resized = old.metrics == metrics && old.native_theme == native_theme;
        old.viewport = [width, height];
        old.metrics = metrics;
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
                    value
                        .extras
                        .as_ref()
                        .is_some_and(|extras| extras.animations.is_some())
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
    let same = match (&theme, &style_cache.environment.theme) {
        (Some(new), Some(old)) => Rc::ptr_eq(new, old) || **new == **old,
        (new, old) => new.is_none() && old.is_none(),
    };
    if !same {
        style_cache.environment.theme = theme.clone();
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
        Some(previous) => out.start_from(previous, count),
        None => out.lengths.reset(count, Lengths::new(16., width, height)),
    }
    let (style, vars) = empty();
    out.values.resize(count, style);
    out.vars.resize(count, vars);
    out.flags.resize(count, 0);
    out.containers.resize(count, None);
    let mut orphans = std::mem::take(&mut style_cache.orphans);
    if let Some(previous) = previous.filter(|_| fast)
        && let Some(nodes) = &previous.anchors
    {
        for id in nodes.boxes.iter().copied().filter(|&id| id < count) {
            if let Some(own) = previous.authored.get(&id) {
                out.values[id] = own.clone();
            }
        }
    }
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
    let (recount, counters) = NodePass {
        doc,
        context,
        walk: &walk,
        scopes: &scopes,
        shadows: &shadows,
        previous,
        registry: &registry,
        theme,
        text_system,
        viewport: [width, height],
        scale,
        native_theme,
        fast,
        stored,
    }
    .run(style_cache, &mut out, &mut orphans);
    out.pass = style_cache.stamp;
    if stored {
        if !fast {
            style_cache.matches.clean_all();
        }
        style_cache.full = false;
        style_cache.styled = Some(Styled {
            nodes: walk.walk.snapshot(),
            serial: walk.walk.serial,
            tree: walk.tree.clone(),
            trees: walk.trees.clone(),
            environment,
            scopes: used,
        });
    }
    number_lists(doc, &walk, &mut out, previous, fast, recount, counters);
    style_cache.collapse_tables(doc, &walk, &mut out, [width, height], previous, fast);
    anonymous_tables(doc, &walk, &shadows, &mut out, previous, fast, &mut orphans);
    style_cache.orphans = orphans;
    if style_cache.anchoring {
        style_cache.anchor(&walk, &mut out, previous.filter(|_| fast));
    }
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

impl StyleCache {
    // Finds the nodes of anchor positioning, and gives the boxes the styles that the last layout found for them.
    fn anchor(
        &mut self,
        walk: &TreeWalk,
        out: &mut ComputedStyles,
        previous: Option<&ComputedStyles>,
    ) {
        let old = previous.and_then(|previous| Some((previous, previous.anchors.as_ref()?)));
        let mut nodes = old.map_or_else(Default::default, |(_, nodes)| nodes.clone());
        let mut add = |id: usize, style: &Style| {
            let boxed = crate::css::value::anchor::positions(style);
            let named = style
                .get(Prop::ANCHOR_NAME)
                .is_some_and(|name| name != "none");
            if (nodes.boxes.contains(&id), nodes.names.contains(&id)) != (boxed, named) {
                let nodes = Rc::make_mut(&mut nodes);
                for (set, has) in [(&mut nodes.boxes, boxed), (&mut nodes.names, named)] {
                    match has {
                        true => set.insert(id),
                        false => set.remove(&id),
                    };
                }
            }
        };
        match old {
            Some((previous, _)) => {
                for id in out.values.differing(&previous.values).flatten() {
                    add(id, &out.values[id]);
                }
            }
            None => {
                for &(id, _) in walk.walk.order().iter() {
                    add(id as usize, &out.values[id as usize]);
                }
            }
        }
        self.anchored_styles
            .retain(|id, _| nodes.boxes.contains(id));
        for &id in &nodes.boxes {
            let Some(used) = self.anchored.get(&id) else {
                continue;
            };
            let own = out.values[id].clone();
            let kept = self
                .anchored_styles
                .get(&id)
                .filter(|(base, with, _)| Rc::ptr_eq(base, &own) && Rc::ptr_eq(with, used));
            let style = match kept {
                Some((.., style)) => style.clone(),
                None => {
                    let mut style = (*own).clone();
                    used.apply(&mut style, false);
                    let style = Rc::new(style);
                    self.anchored_styles
                        .insert(id, (own.clone(), used.clone(), style.clone()));
                    style
                }
            };
            out.values[id] = style;
            out.authored.entry(id).or_insert(own);
        }
        // A style that the cache shares turns it on again.
        self.anchoring = !nodes.boxes.is_empty();
        out.anchors = self.anchoring.then_some(nodes);
        out.anchored = self.anchored.clone();
    }

    // Puts the declarations of the `@position-try` rule `name` into the style of the node `id`, if there is such a rule.
    pub(crate) fn position_try(
        &self,
        doc: &Document,
        (id, name): (usize, &str),
        style: &mut Style,
        parent: Option<&Style>,
        styles: &ComputedStyles,
    ) -> Option<()> {
        let root = std::iter::successors(Some(id), |&at| doc.nodes[at].parent).last()?;
        let scopes = &self.styled.as_ref()?.scopes;
        let scope = scopes.iter().find(|scope| scope.0 == root)?.1;
        let scope = self.scopes.iter().find(|found| found.id == scope)?;
        let (_, values) = scope.keyframes.get(&format!("\0{name}"))?.first()?;
        let mut cascade = bbtree::unsync::BBTreeMap::new();
        for (prop, value, important, position) in values.resolved(false) {
            let rank = rank::CascadeRank {
                position,
                ..rank::CascadeRank::new(rank::AUTHOR, important, 0, [0; 3], 0)
            };
            rank::put(&mut cascade, physical(prop, style), rank, value);
        }
        let mut values = Style::new();
        values.set_vars(styles.vars[id].clone());
        for (&prop, (_, value)) in &cascade {
            values.insert(prop, value.clone());
        }
        resolve_style(
            &mut values,
            parent,
            None,
            &[],
            &styles.registry,
            styles.theme,
        );
        resolve_keywords(&mut values, parent, None);
        for &prop in cascade.keys() {
            match values.value(prop) {
                Some(value) => style.insert(prop, value.clone()),
                None => drop(style.remove(prop)),
            }
        }
        Some(())
    }
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
        anchored: style_cache.anchored.clone(),
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
        let (full_extras, extras) = (full.extras(id), out.extras(id));
        let same = [
            groups(full_extras.slotted.as_ref(), extras.slotted.as_ref()),
            groups(full_extras.parts.as_ref(), extras.parts.as_ref()),
            groups(full_extras.exported.as_ref(), extras.exported.as_ref()),
            *full.values[id] == *out.values[id],
            full.vars[id] == out.vars[id],
            full.flags[id] == out.flags[id],
            full.lengths[id] == out.lengths[id],
            full_extras.generated == extras.generated,
            full_extras.text == extras.text,
            full_extras.backdrop == extras.backdrop,
            full_extras.backdrop_start == extras.backdrop_start,
            full_extras.scrollbars == extras.scrollbars,
            full_extras.selection == extras.selection,
            format!("{:?}", full_extras.animations) == format!("{:?}", extras.animations),
            full_extras.important == extras.important,
            full_extras.starting == extras.starting,
            full.containers[id] == out.containers[id],
        ];
        assert!(
            same.iter().all(|same| *same),
            "incremental style of node {id} <{}> differs from a full pass: {same:?}",
            doc.nodes[id].tag
        );
    }
}
