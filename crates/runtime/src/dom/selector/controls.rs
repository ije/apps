use crate::dom::Document;
use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;

pub(super) fn radio(node: &crate::dom::Node) -> bool {
  node.tag == "input" && node.input_type().eq_ignore_ascii_case("radio")
}

// Whether a node belongs in the index: a form with an id, a radio button, or an element with a `form` attribute.
pub(super) fn indexed(node: &crate::dom::Node) -> bool {
  match node.tag.as_str() {
    "form" => node.attrs.contains_key("id"),
    "input" => radio(node) || node.attrs.contains_key("form"),
    _ => node.attrs.contains_key("form"),
  }
}

fn root(doc: &Document, mut id: usize) -> usize {
  while let Some(parent) = doc.nodes[id].parent {
    id = parent;
  }
  id
}

fn ancestors(doc: &Document, id: usize) -> Vec<usize> {
  let mut chain: Vec<usize> = std::iter::successors(Some(id), |&at| doc.nodes[at].parent).collect();
  chain.reverse();
  chain
}

// Tree order of two nodes of one tree. `read` gets each node whose place counts, and whether its children count.
pub(super) fn precedes(
  doc: &Document,
  a: usize,
  b: usize,
  mut read: impl FnMut(usize, bool),
) -> bool {
  let (a, b) = (ancestors(doc, a), ancestors(doc, b));
  let same = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
  for &node in a[same..].iter().chain(&b[same..]) {
    read(node, false);
  }
  let Some(parent) = same.checked_sub(1).map(|at| a[at]) else {
    return false;
  };
  read(parent, true);
  match (a.get(same), b.get(same)) {
    (Some(a), Some(b)) => {
      let children = &doc.nodes[parent].children;
      children.iter().find(|&child| child == a || child == b) == Some(a)
    }
    (None, _) => true,
    _ => false,
  }
}

// The name of a radio group with its form, or its root without one.
type Group = (String, usize);

#[derive(Default)]
struct Control {
  // The id of a form.
  id: Option<String>,
  // The `form` attribute of another element, and the form that it names.
  form: Option<String>,
  owner: Option<usize>,
  // The form whose `outside` list holds the element.
  outside: Option<usize>,
  group: Option<Group>,
  checked: bool,
}

// The owners of the elements with a `form` attribute, and the radio groups. Styles read them through the children
// kind of the nodes that `update` returns.
#[derive(Default)]
pub(super) struct Controls {
  // Made by `of`.
  pub(super) whole: bool,
  nodes: FxHashMap<usize, Control>,
  forms: FxHashMap<String, Vec<usize>>,
  named: FxHashMap<String, Vec<usize>>,
  // The ids that more than one form has: the first form in tree order counts.
  shared: Vec<String>,
  outside: FxHashMap<usize, Vec<usize>>,
  groups: FxHashMap<Group, (Vec<usize>, usize)>,
}

// Whether the node is in the contents of a template that no script has read.
fn templated(doc: &Document, id: usize) -> bool {
  std::iter::successors(doc.nodes[id].parent, |&at| doc.nodes[at].parent)
    .any(|at| doc.nodes[at].tag == "template")
}

fn take<K: std::hash::Hash + Eq>(lists: &mut FxHashMap<K, Vec<usize>>, key: &K, id: usize) {
  if let Some(list) = lists.get_mut(key) {
    if let Some(at) = list.iter().position(|&other| other == id) {
      list.swap_remove(at);
    }
    if list.is_empty() {
      lists.remove(key);
    }
  }
}

impl Controls {
  pub(super) fn is_empty(&self) -> bool {
    self.nodes.is_empty()
  }

  pub(super) fn contains(&self, id: usize) -> bool {
    self.nodes.contains_key(&id)
  }

  pub(super) fn ids(&self) -> impl Iterator<Item = usize> + '_ {
    self.nodes.keys().copied()
  }

  // All controls in the tree of `id`.
  pub(super) fn of(doc: &Document, id: usize) -> Self {
    let mut found = Vec::new();
    let mut pending = vec![root(doc, id)];
    while let Some(at) = pending.pop() {
      let node = &doc.nodes[at];
      if indexed(node) {
        found.push(at);
      }
      pending.extend(node.children.iter().rev());
    }
    let mut all = Self::default();
    all.update(doc, found, |_| true, false);
    all.whole = true;
    all
  }

  pub(super) fn owner(&self, id: usize) -> Option<usize> {
    self.nodes.get(&id)?.owner
  }

  // The controls that a `form` attribute puts in the form.
  pub(super) fn outside(&self, form: usize) -> &[usize] {
    self.outside.get(&form).map_or(&[], Vec::as_slice)
  }

  // Whether a radio button of the group of `id` is checked.
  pub(super) fn checked(&self, id: usize) -> bool {
    self
      .nodes
      .get(&id)
      .and_then(|control| self.groups.get(control.group.as_ref()?))
      .is_some_and(|group| group.1 > 0)
  }

  // Takes the new state of the nodes in `touched`, and of all that depends on the place of forms when the tree
  // changed. Returns the nodes with another owner, other controls from outside, or another group state.
  pub(super) fn update(
    &mut self,
    doc: &Document,
    mut touched: Vec<usize>,
    alive: impl Fn(usize) -> bool,
    moved: bool,
  ) -> Vec<usize> {
    if touched.is_empty() && (!moved || self.shared.is_empty()) {
      return Vec::new();
    }
    let mut names = match moved {
      true => self.shared.clone(),
      false => Vec::new(),
    };
    touched.sort_unstable();
    touched.dedup();
    let mut found = Vec::new();
    for id in touched {
      let node =
        (doc.nodes.get(id)).filter(|node| alive(id) && indexed(node) && !templated(doc, id));
      if node.is_none() && !self.nodes.contains_key(&id) {
        continue;
      }
      let is_form = node.is_some_and(|node| node.tag == "form");
      let attr = |name: &str| node.and_then(|node| node.attrs.get(name)).cloned();
      let (new_id, form) = match is_form {
        true => (attr("id"), None),
        false => (None, attr("form")),
      };
      let old = self.nodes.entry(id).or_default();
      if old.id != new_id {
        if let Some(name) = old.id.take() {
          take(&mut self.forms, &name, id);
          names.push(name);
        }
        if let Some(name) = &new_id {
          self.forms.entry(name.clone()).or_default().push(id);
          names.push(name.clone());
        }
        old.id = new_id;
      } else if moved {
        names.extend(new_id);
      }
      if old.form != form {
        if let Some(name) = &old.form {
          take(&mut self.named, name, id);
        }
        if let Some(name) = &form {
          self.named.entry(name.clone()).or_default().push(id);
        }
        old.form = form;
      }
      found.push((id, node.is_some()));
    }
    names.sort_unstable();
    names.dedup();
    for name in names {
      let count = self.forms.get(&name).map_or(0, Vec::len);
      match (count > 1, self.shared.contains(&name)) {
        (true, false) => self.shared.push(name.clone()),
        (false, true) => self.shared.retain(|other| *other != name),
        _ => {}
      }
      let named = self.named.get(&name).into_iter().flatten();
      found.extend(named.map(|&id| (id, true)));
    }
    // A removed node stays removed when another form also names it.
    found.sort_unstable();
    found.dedup_by_key(|entry| entry.0);
    let mut changed: Vec<usize> = Vec::new();
    let mut states: FxHashMap<Group, bool> = FxHashMap::default();
    for (id, keep) in found {
      let mut control = self.nodes.remove(&id).unwrap_or_default();
      let node = doc.nodes.get(id).filter(|_| keep);
      let form = match (node, &control.form) {
        (Some(_), Some(name)) => {
          let root = root(doc, id);
          let forms = self.forms.get(name).into_iter().flatten().copied();
          let same = forms.filter(|&form| self::root(doc, form) == root);
          same.reduce(|a, b| if precedes(doc, b, a, |_, _| ()) { b } else { a })
        }
        (Some(node), None) if radio(node) => {
          let up = std::iter::successors(node.parent, |&at| doc.nodes[at].parent);
          up.into_iter().find(|&at| doc.nodes[at].tag == "form")
        }
        _ => None,
      };
      if control.owner != form {
        control.owner = form;
        changed.push(id);
      }
      let outside = form.filter(|_| control.form.is_some());
      if control.outside != outside {
        changed.extend(control.outside.into_iter().chain(outside));
        if let Some(old) = control.outside {
          take(&mut self.outside, &old, id);
        }
        if let Some(form) = outside {
          self.outside.entry(form).or_default().push(id);
        }
        control.outside = outside;
      }
      let group = node.filter(|node| radio(node)).and_then(|node| {
        let name = node.attrs.get("name").filter(|name| !name.is_empty())?;
        Some((name.clone(), form.unwrap_or_else(|| root(doc, id))))
      });
      let checked = node.is_some_and(crate::dom::Node::checked);
      if control.group != group || control.checked != checked {
        if control.group != group {
          changed.push(id);
        }
        if let Some(old) = control.group.take()
          && let Entry::Occupied(mut entry) = self.groups.entry(old.clone())
        {
          states.entry(old).or_insert(entry.get().1 > 0);
          entry.get_mut().0.retain(|&other| other != id);
          entry.get_mut().1 -= usize::from(control.checked);
          if entry.get().0.is_empty() {
            entry.remove();
          }
        }
        if let Some(new) = group {
          let entry = self.groups.entry(new.clone()).or_default();
          states.entry(new.clone()).or_insert(entry.1 > 0);
          entry.0.push(id);
          entry.1 += usize::from(checked);
          control.group = Some(new);
        }
        control.checked = checked;
      }
      if keep {
        self.nodes.insert(id, control);
      }
    }
    for (group, before) in states {
      if let Some((members, checked)) = self.groups.get(&group)
        && (*checked > 0) != before
      {
        changed.extend(members);
      }
    }
    changed.sort_unstable();
    changed.dedup();
    changed
  }
}
