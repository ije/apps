// One style pass over the nodes of a walk: matching, cascade, used values, pseudo-elements and the store.
use crate::css::properties::{Prop, Registry, Style, Vars};
use crate::css::sheet::Declarations;
use crate::css::value::color::color;
use crate::css::value::units::{FontUnits, Lengths};
use crate::dom::Document;
use crate::style::cascade::computed::{
    AGENT_DISPLAY, CAPTION, CELL, COUNTED, ComputedStyles, DISPLAY_CONTENTS, DISPLAY_NONE,
    INVISIBLE, SELECT_ALL, SELECT_NONE, SELECT_TEXT, TABLE_DISPLAYS, empty, table_box,
};
use crate::style::cascade::container::{ContainerQuery, query_containers};
use crate::style::cascade::counters::{list_item, listed};
use crate::style::cascade::memo::{CachedNode, CachedStyle, Inputs, Share, StyleKey, StyleRef};
use crate::style::cascade::moves::mark_moves;
use crate::style::cascade::rank::{
    AGENT, ANIMATION, AUTHOR, CascadeRank, HINT, UNLAYERED, physical, put,
};
use crate::style::cascade::scope::{MatchedRules, PSEUDO, SELECTOR, intern};
use crate::style::cascade::shadow::{
    ELEMENT, PART, SLOTTED, STARTING, ShadowRule, Shadowed, exported_parts,
};
use crate::style::cascade::used_values::Element;
use crate::style::cascade::{Container, Passed, TreeWalk};
use crate::style::keyframes::Animation;
use crate::style::ua::svg_transform;
use crate::style::used::{blockify, resolve_keywords};
use crate::style::vars::{references, resolve_style, var};
use crate::style::{Shadows, StyleCache, StyleContext, drawable_canvas, presentation};
use bbtree::unsync::BBTreeMap;
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
pub(crate) struct NodePass<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) context: Option<&'a StyleContext>,
    pub(crate) walk: &'a TreeWalk,
    pub(crate) scopes: &'a [usize],
    pub(crate) shadows: &'a Shadows,
    pub(crate) previous: Option<&'a ComputedStyles>,
    pub(crate) registry: &'a Rc<Registry>,
    pub(crate) theme: Option<Rc<Style>>,
    pub(crate) text_system: Option<&'a renderer::TextSystem>,
    pub(crate) viewport: [f32; 2],
    pub(crate) scale: f32,
    pub(crate) native_theme: u32,
    pub(crate) fast: bool,
    pub(crate) stored: bool,
}

impl NodePass<'_> {
    // Styles the dirty nodes and the nodes below them, or all nodes. Returns whether list numbers must be counted
    // again, and whether any node uses counters.
    pub(crate) fn run(
        &self,
        style_cache: &mut StyleCache,
        out: &mut ComputedStyles,
        orphans: &mut Vec<usize>,
    ) -> (bool, bool) {
        let (doc, context, walk, scopes, shadows, previous, registry, theme) = (
            self.doc,
            self.context,
            self.walk,
            self.scopes,
            self.shadows,
            self.previous,
            self.registry,
            &self.theme,
        );
        let (_text_system, [width, height], _scale, native_theme, fast, stored) = (
            self.text_system,
            self.viewport,
            self.scale,
            self.native_theme,
            self.fast,
            self.stored,
        );
        let order = walk.walk.nodes.as_slice();
        // The generation and revision of each dependency key in the list of the current node.
        let (mut unique, mut generation) = (Vec::<(u32, u32)>::new(), 0u32);
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
        let mut changed = std::mem::take(&mut style_cache.changed);
        style_cache.stamp += 1;
        let stamp = style_cache.stamp << 8;
        // A fast pass numbers the counters again only when one of their inputs changed.
        let mut recount = false;
        if fast {
            mark_moves(
                doc,
                walk,
                shadows,
                style_cache.styled.as_ref(),
                matches,
                out,
                &mut changed,
                &mut recount,
                (&unstyled, &empty),
            );
        }
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
        } = out;
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
        macro_rules! changed {
            ($id:expr) => {
                match changed[$id] ^ stamp {
                    value if value < 256 => value as u8,
                    _ => 0,
                }
            };
        }
        let mut counters = false;
        let html: Vec<usize> = walk.special[0]
            .iter()
            .copied()
            .filter(|&id| doc.nodes[id].tag == "html")
            .collect();
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
        let mut texts =
            FxHashMap::<_, (Rc<Style>, Vars, u16, Lengths, Option<Rc<Style>>)>::default();
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
                    _ if cfg!(debug_assertions) => {
                        reached = seeds.get(seed) == Some(&(next as u32))
                    }
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
                if !inner.nodes[root].as_ref().is_some_and(|matched| {
                    matches.valid_since(&matched.dependencies, &matched.checked)
                }) {
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
                for (kind, order, specificity, pseudo) in
                    inner.nodes[root].iter().flat_map(|matched| {
                        matched
                            .rules
                            .iter()
                            .map(|&(order, specificity)| (ELEMENT, order, specificity, 0))
                            .chain(matched.pseudos.iter().map(|&index| {
                                let (kind, selector, order, _) = &inner.pseudos[index];
                                (*kind, *order, selector.specificity, index)
                            }))
                    })
                {
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
            let scope = scoped.id;
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
                        (value.missing.iter().all(|name| {
                            Prop::find(name).is_none_or(|prop| var(new, prop).is_none())
                        }) && reads
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
                    let (matched, matched_pseudos) = scoped.match_node(
                        doc,
                        id,
                        matches,
                        group,
                        &mut shares,
                        &mut candidates,
                        (&mut unique, &mut generation),
                        &no_pseudos,
                    );
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
                let (_, declarations, layer) = &scoped.rules[order];
                for &(prop, ref value, important, position) in declarations.iter() {
                    let rank = CascadeRank::rule(
                        *layer,
                        important,
                        specificity,
                        scoped.keys[order],
                        position,
                    );
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
                    let (_, declarations, rule_layer) = &scoped.rules[order];
                    let from = rule_layer.origin.min(HINT);
                    if from > origin
                        || from == origin && (revert || !rank.inline && rule_layer.rank == layer)
                    {
                        continue;
                    }
                    for (_, candidate, important, position) in
                        declarations.iter().filter(|(name, ..)| {
                            *name == prop || *name == Prop::ALL && !prop.is_custom()
                        })
                    {
                        let candidate_rank = CascadeRank::rule(
                            *rule_layer,
                            *important,
                            specificity,
                            scoped.keys[order],
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
                if let Some((_, value)) = key.hints.iter().find(|(prop, _)| *prop == Prop::OVERLAY)
                {
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
                registry,
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
                for prop in crate::css::properties::PADDING
                    .into_iter()
                    .chain(
                        crate::css::properties::BORDERS
                            .into_iter()
                            .flat_map(|side| [side[0], side[1]]),
                    )
                    .chain([Prop::BACKGROUND_COLOR, Prop::BACKGROUND_IMAGE])
                {
                    style.remove(prop);
                }
            }
            let (role, units, queried, viewport) = self.used_values(
                &mut style,
                Element {
                    id,
                    node,
                    parent_style,
                    parent,
                    parent_role,
                    item,
                    table_item,
                    seen: &seen,
                },
                explicit_size,
                &mut style_cache.sized,
                &style_cache.containers,
                &mut font_metrics,
                &mut root,
            );
            let mut pseudo_cascade: [BBTreeMap<Prop, (CascadeRank, &Rc<str>)>; 17] =
                Default::default();
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
                                !scoped.medias[*order].as_ref().is_some_and(|stack| {
                                    stack.iter().any(|block| !block.matched.get())
                                }) && query(conditions, (scope, *order))
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
                let (_, declarations, layer) = &scoped.rules[*order];
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
                                order: scoped.keys[*order],
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
                        scoped.keys[*order],
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
                    registry,
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
                        registry,
                        native_theme,
                    );
                    resolve_keywords(&mut values, Some(&style), Some(&units));
                    if !keep && declared.is_empty() {
                        values.set_vars(empty.clone());
                    }
                    values
                })
            };
            let layered = node.state & (crate::dom::TOP_LAYER | crate::dom::LEAVING) != 0
                && node.tag != "html";
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
                    registry,
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
                        Animation::parse(&style, &scoped.keyframes, registry, native_theme)
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
                            Animation::parse(pseudo, &scoped.keyframes, registry, native_theme)
                                .into_iter()
                                .map(|mut effect| {
                                    effect.pseudo = slot as u8 + 1;
                                    effect.name = format!(
                                        "{}\0{}",
                                        ["::before", "::after"][slot],
                                        effect.name
                                    );
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
                Some(old) if !Rc::ptr_eq(old, style.vars()) && **old == **style.vars() => {
                    old.clone()
                }
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
        (recount, counters)
    }
}
