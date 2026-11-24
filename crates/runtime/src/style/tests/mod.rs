use super::*;
use crate::css::{
    Edit, Filter, MARGIN, MediaQuery, PADDING, Registry, clip_path, counter_text, declarations,
    declare, length, resolved, specified, supports, supports_condition, syntax_matches,
};
use crate::style::cascade::rank::CascadeRank;
use crate::style::cascade::scope::{PSEUDO, SELECTOR};
use rustc_hash::FxHashMap;
use std::rc::Rc;

mod cascade;
mod containers;
mod glass;
mod incremental;
mod shadow;
mod tables;
mod tailwind;
mod text;
mod values;

fn element(doc: &Document, selector: &str) -> usize {
    (0..doc.nodes.len())
        .find(|&id| doc.matches(id, selector))
        .unwrap()
}

fn typed(values: &BBTreeMap<String, String>) -> Style {
    values
        .iter()
        .map(|(name, value)| (Prop::new(name), value.as_str().into()))
        .collect()
}

fn styles_of(doc: &Document, selector: &str, name: &str) -> Option<String> {
    styles(doc, 800., 600.)[element(doc, selector)]
        .get(name)
        .cloned()
}

fn attach(doc: &mut Document) -> Rc<Shadows> {
    let mut pairs = Vec::new();
    for (host, root) in [("#host", "#root"), ("#inner", "#root2")] {
        let Some(root) = (0..doc.nodes.len()).find(|&id| doc.matches(id, root)) else {
            continue;
        };
        if let Some(parent) = doc.nodes[root].parent {
            doc.nodes[parent].children.retain(|&id| id != root);
        }
        pairs.push((element(doc, host), root));
        let node = &mut doc.nodes[root];
        node.parent = None;
        node.tag = "#fragment".into();
        node.attrs = Default::default();
    }
    let host = pairs[0].0;
    let mut composed: FxHashMap<usize, Vec<usize>> = pairs
        .iter()
        .map(|&(host, root)| (host, vec![root]))
        .collect();
    if let Some(slot) = doc.nodes.iter().position(|node| node.tag == "slot") {
        let assigned = doc.nodes[host].children.iter();
        composed.insert(
            slot,
            assigned.chain(&doc.nodes[slot].children).copied().collect(),
        );
    }
    Rc::new(Shadows {
        roots: pairs.iter().copied().collect(),
        hosts: Rc::new(pairs.iter().map(|&(host, root)| (root, host)).collect()),
        composed,
    })
}

fn incremental(
    doc: &Document,
    cache: &mut StyleCache,
    last: &mut Option<Rc<ComputedStyles>>,
    viewport: [f32; 2],
) -> Rc<ComputedStyles> {
    let context = StyleContext {
        previous: last.clone(),
        ..Default::default()
    };
    let out = Rc::new(compute_styles(
        doc,
        viewport[0],
        viewport[1],
        None,
        &mut HashMap::new(),
        cache,
        Some(&context),
    ));
    cache.finish();
    *last = Some(out.clone());
    out
}

fn shadowed(
    doc: &Document,
    shadows: &Rc<Shadows>,
    cache: &mut StyleCache,
    last: &mut Option<Rc<ComputedStyles>>,
) -> Rc<ComputedStyles> {
    let context = StyleContext {
        shadows: shadows.clone(),
        previous: last.clone(),
        ..Default::default()
    };
    let out = Rc::new(compute_styles(
        doc,
        800.,
        600.,
        None,
        &mut HashMap::new(),
        cache,
        Some(&context),
    ));
    cache.finish();
    *last = Some(out.clone());
    out
}
