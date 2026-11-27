use crate::css::selector::{ACTIVE, FOCUS, FOCUS_VISIBLE, FOCUS_WITHIN, HOVER, Selector};
use crate::dom::SelectorCache;
use crate::dom::{Document, Node};
use crate::render::PageState;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
  collections::{HashMap, HashSet},
  rc::Rc,
};

fn parsed(state: &mut PageState, source: &Value) -> Result<Rc<[Selector]>> {
  let source = source.as_str().context("Expected selector")?;
  if let Some(found) = state.styles.queries.get(source) {
    return Ok(found.clone());
  }
  let parsed: Rc<[Selector]> = Selector::try_parse(source)?.into();
  // Scripts repeat a few selectors, so a full cache holds generated ones.
  if state.styles.queries.len() == 256 {
    state.styles.queries.clear();
  }
  state.styles.queries.insert(source.into(), parsed.clone());
  Ok(parsed)
}

fn tree(doc: &Document, root: usize, inclusive: bool) -> Value {
  let mut list = Vec::with_capacity(1024);
  let mut pending: Vec<_> = if inclusive {
    vec![root]
  } else {
    doc.nodes[root].children.iter().rev().copied().collect()
  };
  while let Some(id) = pending.pop() {
    let node = &doc.nodes[id];
    let lazy = node.stylesheet.is_some() && node.tag == "#text"
      || !node.text.is_empty()
        && node
          .parent
          .is_some_and(|parent| matches!(doc.nodes[parent].tag.as_str(), "script" | "style"));
    let text = (!lazy).then_some(&node.text);
    let count = node.attrs.iter().count();
    let at = list.len();
    let _ = serde_json::to_writer(&mut list, &(id, node.parent, &node.tag, text, count));
    if at > 0 {
      list[at] = b',';
    }
    list.pop();
    for pair in &node.attrs {
      let at = list.len();
      let _ = serde_json::to_writer(&mut list, &pair);
      list[at] = b',';
      list.pop();
    }
    pending.extend(node.children.iter().rev());
  }
  if list.is_empty() {
    list.push(b'[');
  }
  list.push(b']');
  Value::String(String::from_utf8(list).unwrap_or_default())
}

pub(super) fn release(args: &Value, doc: &mut Document, state: &mut PageState) -> Result<Value> {
  let wrapped = args[1].as_array().context("Expected live nodes")?;
  state.tree.note_created(wrapped.len());
  let dead = args[0].as_array().context("Expected dead nodes")?;
  let ids = |value: &Value| value.as_u64().map(|id| id as usize);
  state.tree.mark_live(
    doc,
    wrapped.iter().filter_map(ids),
    dead.iter().filter_map(ids),
  );
  let (canvases, freed) = state.release(doc);
  Ok(json!([canvases, freed]))
}

pub(super) fn focus(
  op: &str,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let ids = args[0]
    .as_array()
    .context("Expected nodes")?
    .iter()
    .map(|id| {
      id.as_u64()
        .map(|id| id as usize)
        .filter(|&id| id < doc.nodes.len())
        .context("Invalid node id")
    })
    .collect::<Result<Vec<_>>>()?;
  let (index, flag) = match op {
    "dom.focus" => (0, FOCUS_WITHIN),
    "dom.hover" => (1, HOVER),
    _ => (2, ACTIVE),
  };
  let focus = if op == "dom.focus" { FOCUS } else { 0 };
  let shown = args[2].as_bool() != Some(true);
  let used = state.styles.cache.states & (flag | focus) != 0;
  let control = |id: &usize| {
    doc.nodes.get(*id).is_some_and(|node| {
      matches!(
        node.tag.as_str(),
        "button" | "input" | "select" | "textarea"
      )
    })
  };
  let repaint =
    focus == 0 && (ids.iter().any(control) || state.interaction.states[index].iter().any(control));
  if op == "dom.focus" {
    for &id in state
      .interaction
      .focus
      .iter()
      .chain(&state.interaction.states[0])
    {
      if let Some(node) = doc.nodes.edit(id) {
        node.state &= !(FOCUS | FOCUS_VISIBLE);
      }
    }
    state.interaction.focus = ids.first().copied();
    if let Some(focus) = state.interaction.focus.filter(|_| shown) {
      if let Some(node) = doc.nodes.edit(focus) {
        node.state |= if args[1].as_bool() == Some(false) {
          FOCUS
        } else {
          FOCUS | FOCUS_VISIBLE
        };
      }
      // Like Chrome, the hosts of the shadow trees around the focused element match `:focus`. A host of a
      // slotted element does not, so the walk follows tree parents, not the composed path.
      let mut at = focus;
      while let Some(host) = {
        while let Some(up) = doc.nodes[at].parent {
          at = up;
        }
        ids
          .iter()
          .copied()
          .find(|host| state.tree.shadows.get(host) == Some(&at))
      } {
        if let Some(node) = doc.nodes.edit(host) {
          node.state |= FOCUS;
        }
        at = host;
      }
    }
  }
  for &id in &state.interaction.states[index] {
    if let Some(node) = doc.nodes.edit(id) {
      node.state &= !flag;
    }
  }
  for &id in ids.iter().filter(|_| shown) {
    if let Some(node) = doc.nodes.edit(id) {
      node.state |= flag;
    }
  }
  if index == 0 {
    for handle in state.geometry.scroll.values() {
      handle.focus_snaps(&ids);
    }
  }
  state.interaction.states[index] = ids;
  if used {
    state.styles.invalidate();
  }
  state.dirty |= used || focus != 0 || repaint;
  Ok(Value::Null)
}

pub(super) fn target(args: &Value, doc: &mut Document, state: &mut PageState) -> Result<Value> {
  let target = args[0]
    .as_u64()
    .map(|id| id as usize)
    .filter(|&id| id < doc.nodes.len());
  for id in [doc.target, target].into_iter().flatten() {
    doc.nodes.edit(id);
  }
  doc.target = target;
  state.styles.invalidate();
  state.dirty = true;
  Ok(Value::Null)
}

pub(super) fn reserve(args: &Value, doc: &mut Document, state: &mut PageState) -> Result<Value> {
  let ids: Vec<_> = (0..args[0].as_u64().context("Expected count")?.min(4096))
    .map(|_| state.allocate(doc, Node::default()))
    .collect();
  state.tree.reserve(&ids);
  Ok(json!(ids))
}

pub(super) fn assigned_nodes(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let mut roots = HashSet::new();
  let mut slotted = HashMap::new();
  let mut add = |slot: usize, slotted: &mut HashMap<usize, Vec<usize>>| {
    let mut top = slot;
    while let Some(parent) = doc.nodes[top].parent {
      top = parent;
    }
    if let Some(&host) = state.tree.hosts.get(&top)
      && roots.insert(top)
    {
      slotted.extend(crate::dom::tree::slots(doc, [(host, top)]));
    }
  };
  add(id, &mut slotted);
  let mut nodes = slotted.get(&id).cloned().unwrap_or_default();
  if args[1].as_bool() == Some(true) && slotted.contains_key(&id) {
    if nodes.is_empty() {
      nodes = doc.nodes[id].children.clone();
    }
    let mut pending: Vec<_> = nodes.into_iter().rev().collect();
    nodes = Vec::new();
    while let Some(child) = pending.pop() {
      if doc.nodes[child].tag == "slot" {
        add(child, &mut slotted);
      }
      if let Some(assigned) = slotted.get(&child) {
        pending.extend(
          if assigned.is_empty() {
            &doc.nodes[child].children
          } else {
            assigned
          }
          .iter()
          .rev(),
        );
      } else if doc.nodes[child].tag != "#comment" {
        nodes.push(child);
      }
    }
  }
  Ok(json!(nodes))
}

pub(super) fn subtree(id: usize, doc: &mut Document) -> Result<Value> {
  Ok(tree(doc, id, true))
}

pub(super) fn source(id: usize, doc: &mut Document) -> Result<Value> {
  let node = &doc.nodes[id];
  Ok(json!(match &node.stylesheet {
    Some(sheet) if node.tag == "#text" => sheet.source(),
    _ => node.text.clone(),
  }))
}

pub(super) fn matches(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  Ok(json!(parsed(state, &args[1])?.iter().any(|s| {
    SelectorCache::default().matches(s, doc, id, Some(id))
  })))
}

pub(super) fn closest(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let selectors = parsed(state, &args[1])?;
  let mut memo = SelectorCache::default();
  let mut current = Some(id);
  while let Some(node) = current.filter(|&node| !doc.nodes[node].tag.starts_with('#')) {
    if selectors
      .iter()
      .any(|s| memo.matches(s, doc, node, Some(id)))
    {
      return Ok(json!(node));
    }
    current = doc.nodes[node].parent;
  }
  Ok(Value::Null)
}

pub(super) fn query_within(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let selectors = parsed(state, &args[1])?;
  let first = args[2].as_bool().unwrap_or(false);
  let mut pending: Vec<_> = doc.nodes[id].children.iter().rev().copied().collect();
  let mut found = Vec::new();
  let mut memo = SelectorCache::default();
  while let Some(node) = pending.pop() {
    if selectors
      .iter()
      .any(|s| memo.matches(s, doc, node, Some(id)))
    {
      if first {
        return Ok(json!(node));
      }
      found.push(node);
    }
    if doc.nodes[node].tag != "template" {
      pending.extend(doc.nodes[node].children.iter().rev());
    }
  }
  Ok(if first { Value::Null } else { json!(found) })
}

pub(super) fn html(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let source = args[1].as_str().context("Expected HTML")?;
  let tag = doc.nodes[id].tag.as_str();
  let form = || {
    std::iter::successors(Some(id), |&at| doc.nodes[at].parent)
      .any(|at| doc.nodes[at].tag == "form")
  };
  let parsed = match args[2].as_str() {
    _ if tag == "#document" => Document::parse(source),
    Some(context) => Document::build(source, Some(context), form()),
    // Shadow roots and template contents parse in `template` context, so the `<tr>` rows that
    // Lit renders are kept.
    None if tag.starts_with('#') => Document::fragment(source, "template"),
    None => Document::build(source, Some(tag), form()),
  };
  state.tree.detach_children(doc, id);
  doc.nodes[id].text.clear();
  let mut ids = Vec::with_capacity(parsed.nodes.len());
  ids.push(id);
  for _ in 1..parsed.nodes.len() {
    ids.push(state.tree.slot(doc));
  }
  doc.nodes[id].children = parsed.nodes[0]
    .children
    .iter()
    .map(|&child| ids[child])
    .collect();
  for (index, mut node) in parsed.nodes.into_iter().enumerate().skip(1) {
    node.parent = node.parent.map(|parent| ids[parent]);
    for child in &mut node.children {
      *child = ids[*child];
    }
    state.place_attached(doc, ids[index], node);
  }
  state.dirty = true;
  Ok(tree(doc, id, false))
}
