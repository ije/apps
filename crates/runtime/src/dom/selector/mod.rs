mod controls;
mod matching;
use controls::{Controls, indexed};
pub use matching::matches;
use matching::{has, next_stamp, ranged, reads};
#[cfg(test)]
mod tests;

use super::Document;
use crate::css::selector::Ancestry;
use crate::css::selector::BLOOM;
use crate::css::selector::Bloom;
use crate::css::selector::Selector;
use crate::css::selector::feature;
use crate::css::selector::lowercase;
use std::{
  hash::{BuildHasher, Hash, Hasher},
  rc::Rc,
};

// Match results by selector, node, and scope with a flag: the place of their dependencies in `found`, and the
// result in the top bit of the length.
type Matches = rustc_hash::FxHashMap<(usize, u32, u32), (u32, u32)>;

const MATCHED: u32 = 1 << 31;

fn match_key(
  selector: usize,
  id: usize,
  scope: Option<usize>,
  ancestor: bool,
) -> (usize, u32, u32) {
  let scope = scope.map_or(0, |scope| scope as u32 + 1);
  (selector, id as u32, scope << 1 | u32::from(ancestor))
}

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
// A `watched` bit: another node read the style attribute of the node.
const STYLED: u8 = 32;

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
  attributes: Named,
  nodes: Vec<Snapshot>,
  seen: Vec<u64>,
  // The walk and document revision of the last update, and the nodes forgotten since.
  walked: u64,
  latest: u64,
  forgotten: Vec<usize>,
  values: Matches,
  // The dependencies of `values`. Replaced results leave theirs here until both are cleared.
  found: Vec<Dependency>,
  positions: rustc_hash::FxHashMap<usize, (u32, Positions)>,
  nth: rustc_hash::FxHashMap<(usize, usize, Option<usize>), NthPositions>,
  dependencies: Vec<Dependency>,
  // The last record that held each dependency key, by record generation, for `finish_repeated`.
  unique: Vec<(u32, u32)>,
  generation: u32,
  revision: u32,
  pub(crate) epoch: u64,
  changes: std::collections::VecDeque<(u64, Vec<u32>)>,
  // An incremental pass styles only `dirty` nodes, so every recorded dependency must reach `dependents`.
  dirty: Vec<bool>,
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
  controls: Controls,
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
    self.found.clear();
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
      let (node, kind) = (change as usize / KINDS, change as usize % KINDS);
      // Other nodes read a style attribute only through `[style]` selectors, so a new one restyles the
      // node, and its children only by inheritance.
      if kind == 6
        && self
          .watched
          .get(node)
          .is_some_and(|watched| watched & STYLED == 0)
      {
        self.dirty[node] = true;
        continue;
      }
      self.mark(node, matches!(kind, 2 | EDGE..=INDEX));
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

  // Starts the record of what the matches of a node read.
  pub(crate) fn begin(&mut self) {
    self.dependencies.clear();
  }

  pub(crate) fn recorded(&self) -> &[Dependency] {
    &self.dependencies
  }

  // Ends the record of node `id` like `finish`, for a record where most entries repeat: each candidate
  // selector reads the node again. It drops them in one pass before sorting.
  pub(crate) fn finish_repeated(&mut self, id: usize) {
    self.generation = self.generation.wrapping_add(1);
    if self.generation == 0 {
      self.unique.fill((0, 0));
      self.generation = 1;
    }
    let (unique, generation) = (&mut self.unique, self.generation);
    self.dependencies.retain(|&(key, revision)| {
      let key = key as usize;
      if key >= unique.len() {
        unique.resize(key + 1, (0, 0));
      }
      std::mem::replace(&mut unique[key], (generation, revision)) != (generation, revision)
    });
    self.dependencies.sort_unstable();
    self.register(id);
  }

  // Ends the record of node `id`: its dependencies, once each, mark it when they change.
  pub(crate) fn finish(&mut self, id: usize) {
    self.dependencies.sort_unstable();
    self.dependencies.dedup();
    self.register(id);
  }

  // Marks node `id` for the next incremental pass.
  pub(crate) fn touch(&mut self, id: usize) {
    if let Some(dirty) = self.dirty.get_mut(id) {
      *dirty = true;
    }
  }

  pub(crate) fn is_dirty(&self, id: usize) -> bool {
    self.dirty[id]
  }

  pub(crate) fn clean(&mut self, id: usize) {
    self.dirty[id] = false;
  }

  pub(crate) fn clean_all(&mut self) {
    self.dirty.fill(false);
  }

  // The marked nodes, in order.
  pub(crate) fn dirty_nodes(&self) -> impl Iterator<Item = usize> + '_ {
    self
      .dirty
      .chunks(64)
      .enumerate()
      // Warning: `contains` tests each entry in turn. This `or` of the whole chunk vectorizes.
      .filter(|(_, chunk)| chunk.iter().fold(false, |any, &dirty| any | dirty))
      .flat_map(|(index, chunk)| {
        let set = chunk.iter().enumerate().filter(|(_, dirty)| **dirty);
        set.map(move |(bit, _)| index * 64 + bit)
      })
  }

  // `dependencies` must be sorted, as `chunk_by` groups them by node.
  pub(crate) fn register(&mut self, id: usize) {
    for group in self
      .dependencies
      .chunk_by(|a, b| a.0 as usize / KINDS == b.0 as usize / KINDS)
    {
      let node = group[0].0 as usize / KINDS;
      if node != id
        && group.iter().any(|&(kind, _)| kind as usize % KINDS == 6)
        && let Some(watched) = self.watched.get_mut(node)
      {
        *watched |= STYLED;
      }
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
          let stamp = next_stamp(&mut self.stamp, &mut self.stamps, self.seen.len());
          let (seen, stamps) = (&self.seen, &mut self.stamps);
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
    self.found = Vec::new();
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
    self.blooms.truncate(len);
    self.blooms.shrink_to_fit();
    self.stamps.truncate(len);
    self.stamps.shrink_to_fit();
    self.within.retain(|&anchor, _| anchor < len);
  }

  pub(crate) fn update(&mut self, doc: &Document, walk: &super::Walk) {
    self.pass = self.pass.wrapping_add(1).max(1);
    if self.values.len() > 131072 {
      self.values.clear();
      self.found.clear();
    }
    if self.positions.len() > 65536 {
      self.positions.clear();
    }
    if self.nodes.len() < doc.nodes.len() {
      self.nodes.resize(doc.nodes.len(), Snapshot::default());
      self.seen.resize(doc.nodes.len(), 0);
    }
    self
      .dirty
      .resize(doc.nodes.len().max(self.dirty.len()), true);
    self
      .dependents
      .resize_with(doc.nodes.len().max(self.dependents.len()), Default::default);
    self
      .children
      .resize_with(doc.nodes.len().max(self.children.len()), Default::default);
    self
      .watched
      .resize(doc.nodes.len().max(self.watched.len()), 0);
    let renamed = self.named != self.attributes.generation;
    self.named = self.attributes.generation;
    let mut changes = Vec::new();
    let mut quieted = Vec::new();
    let mut retagged = Vec::new();
    let mut moved = Vec::new();
    let start = self.revision;
    let (mut touched, mut reparented, mut reordered) = (Vec::new(), Vec::new(), false);
    // Since the last update, only a node with a newer revision, a forgotten one or one that a patched walk
    // touched can have another snapshot. Debug builds check all nodes against that.
    let delta = match self.walked == walk.serial {
      true => Some(&[][..]),
      false => walk.since(self.walked),
    };
    if !self.controls.is_empty() {
      let gone = |&id: &usize| self.controls.contains(id) && !walk.contains(id);
      touched.extend(self.forgotten.iter().copied().filter(gone));
      match delta {
        Some(delta) => touched.extend(delta.iter().map(|&id| id as usize).filter(gone)),
        None => touched.extend(self.controls.ids().filter(gone)),
      }
    }
    let listed: Option<Vec<usize>> = delta.map(|touched| {
      let newer = doc.nodes.changed_since(self.latest);
      let touched = touched.iter().map(|&id| id as usize);
      let nodes = newer.chain(touched).chain(self.forgotten.iter().copied());
      nodes.filter(|&id| walk.contains(id)).collect()
    });
    (self.walked, self.latest) = (walk.serial, doc.nodes.latest());
    self.forgotten.clear();
    let mut every = walk.order().iter().map(|node| node.0 as usize);
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
      if indexed(node) || !self.controls.is_empty() && self.controls.contains(id) {
        touched.push(id);
      }
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
            || matches!(
              key.as_str(),
              "key" | "title" | "role" | "tabindex" | "inert"
            );
          (key.as_str() != "style"
            && (key.as_str() != "class" || self.attributes.names.contains("class"))
            && (!selected || self.attributes.names.contains(key.as_str())))
          .then(|| {
            (
              key,
              (!selected || self.attributes.valued.contains(key.as_str())).then_some(value),
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
                    // The direction of the text, for `dir=auto`.
                    | (renderer::right_to_left(node.current_value().unwrap_or(&node.text)))
                        .map_or(0, |rtl| 8 << u32::from(rtl))
                    | node.value.as_deref().map_or(0, |value| {
                        // Ranged inputs and custom states match by their value, other inputs only by whether
                        // they have one.
                        if ranged(node) || node.tag.contains('-') {
                            (hash.hash_one(value) as u32) << 5 | 2
                        } else {
                            2 << u32::from(value.is_empty())
                        }
                    }),
        tag as u32,
        node.state | u32::from(doc.target == Some(id)) << 31,
        hash.hash_one((node.attrs.get("style"), &node.inline)) as u32,
        node
          .attrs
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
      let placed = values[1] != 0;
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
              reordered = true;
              if placed {
                reparented.push(id);
              }
              moved.extend(node.parent);
              if let Some(results) = self.within.get_mut(&id) {
                results.retain(|result| !result.2);
                if results.is_empty() {
                  self.within.remove(&id);
                }
              }
            }
            2 => {
              reordered = true;
              moved.push(id);
            }
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
    if !self.controls.is_empty() {
      next_stamp(&mut self.stamp, &mut self.stamps, self.seen.len());
      while let Some(id) = reparented.pop() {
        if std::mem::replace(&mut self.stamps[id], self.stamp) != self.stamp {
          if self.controls.contains(id) {
            touched.push(id);
          }
          reparented.extend(&doc.nodes[id].children);
        }
      }
    }
    // These reads use the children kind, which keeps a node from sharing its style.
    for id in (self.controls).update(doc, touched, |id| walk.contains(id), reordered) {
      if let Some(slot) = self.nodes.get_mut(id) {
        self.revision = self.revision.wrapping_add(1);
        slot.revisions[2] = self.revision;
        changes.push((id * KINDS + 2) as u32);
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
      if round == 1 || self.within.is_empty() {
        break;
      }
      // A change can flip the `:has()` results of the nodes above it, and of their children that read
      // siblings.
      next_stamp(&mut self.stamp, &mut self.stamps, self.seen.len());
      let mut above = Vec::new();
      // A local result reads no style attribute or own class list, and a new place follows a change
      // of the parent's children.
      let read = |change: &&u32| !matches!(**change as usize % KINDS, 4 | 6 | 7 | EDGE..);
      let mut kinds = 0;
      for &change in changes.iter().filter(read) {
        kinds |= 1 << (change as usize % KINDS);
        let mut at = Some(change as usize / KINDS);
        while let Some(node) =
          at.filter(|&node| std::mem::replace(&mut self.stamps[node], self.stamp) != self.stamp)
        {
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
            let reads = parsed.iter().fold(6, |all, selector| all | reads(selector));
            if *sibling != (index > 0) || kinds & reads == 0 {
              return true;
            }
            // Only this list holds the arguments of a dropped rule.
            let live = Rc::strong_count(parsed) > 1;
            let now = live && has(parsed, doc, *anchor, self, true).0;
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

  pub(crate) fn reject(&mut self, doc: &Document, id: usize, (_, steps, _): &Ancestry, rest: u16) {
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
      self
        .dependencies
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
    let placed = (id * KINDS + PREVIOUS) as u32;
    let placed = self.dependencies.iter().any(|&(key, _)| key == placed);
    let (siblings, count) = self.previous(id, placed);
    self.dependencies.iter().all(|&(read, _)| {
      let (node, kind) = (read as usize / KINDS, read as usize % KINDS);
      if node == id {
        kind != 2
      } else if siblings[..count].contains(&node) {
        !matches!(kind, 2 | EDGE | INDEX | WITHIN)
      } else {
        !(Some(node) == parent && kind == 2) && chain[..length].contains(&node)
      }
    })
  }

  // The element siblings before `id`, nearest first. Its parent must keep their places, as after a read of the
  // `PREVIOUS` kind of `id`. A share maps the reads of these siblings, like the reads of ancestors.
  fn previous(&self, id: usize, placed: bool) -> ([usize; 4], usize) {
    let (mut siblings, mut count, mut at) = ([0; 4], 0, id);
    if placed {
      while count < siblings.len()
        && let Some(before) = self
          .nodes
          .get(at)
          .and_then(|node| node.values[PREVIOUS].checked_sub(1))
      {
        at = before as usize;
        siblings[count] = at;
        count += 1;
      }
    }
    (siblings, count)
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
    // The siblings before `from` pair with those before `to` at the same distance.
    let placed = (from * KINDS + PREVIOUS) as u32;
    let mut siblings = [([0; 4], 0); 2];
    if dependencies.iter().any(|&(key, _)| key == placed) {
      let bit = 1 << (PREVIOUS - EDGE);
      if let Some(parent) = doc.nodes[to].parent
        && self
          .watched
          .get(parent)
          .is_none_or(|watched| watched & bit == 0)
      {
        self.place(doc, parent, bit, &mut Vec::new());
      }
      siblings = [self.previous(from, true), self.previous(to, true)];
    }
    // Sibling ids differ, so `PREVIOUS` compares only whether there is a sibling. The reads of the siblings
    // compare too, and `shareable` keeps the siblings of ancestors out.
    let differs = |this: &Self, node: usize, other: usize, kind: usize| match kind {
      PREVIOUS => (this.nodes[node].values[kind] == 0) != (this.nodes[other].values[kind] == 0),
      _ => this.nodes[node].values[kind] != this.nodes[other].values[kind],
    };
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
            let (Some(parent), bit) = (doc.nodes[other].parent, 1 << (kind - EDGE)) else {
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
          if !(kind == 1 || kind == 6 && node == from) && differs(self, node, other, kind) {
            return false;
          }
          (
            (other * KINDS + kind) as u32,
            self.nodes[other].revisions[kind],
          )
        }
        None => match siblings[0].0[..siblings[0].1]
          .iter()
          .position(|&sibling| sibling == node)
        {
          Some(index) => {
            if index >= siblings[1].1 || matches!(kind, 2 | EDGE | INDEX | WITHIN) {
              return false;
            }
            let other = siblings[1].0[index];
            if kind != 1 && differs(self, node, other, kind) {
              return false;
            }
            (
              (other * KINDS + kind) as u32,
              self.nodes[other].revisions[kind],
            )
          }
          None if node == top && kind == 2 => return false,
          None => (read, revision),
        },
      };
      self.dependencies.push(dependency);
    }
    self.dependencies.sort_unstable();
    true
  }

  pub(crate) fn valid(&self, dependencies: &[Dependency]) -> bool {
    dependencies.iter().all(|&(id, revision)| {
      let id = id as usize;
      self
        .nodes
        .get(id / KINDS)
        .map_or(0, |node| node.revisions[id % KINDS])
        == revision
    })
  }

  // Keeps a result with the dependencies recorded since `from`.
  fn store(&mut self, key: (usize, u32, u32), result: bool, from: usize) {
    if self.found.len() + self.dependencies.len() - from > 2_000_000 {
      self.values.clear();
      self.found.clear();
    }
    let start = self.found.len();
    self.found.extend_from_slice(&self.dependencies[from..]);
    self.found[start..].sort_unstable();
    let mut end = start;
    for at in start..self.found.len() {
      if end == start || self.found[at] != self.found[end - 1] {
        self.found[end] = self.found[at];
        end += 1;
      }
    }
    self.found.truncate(end);
    let len = (end - start) as u32 | if result { MATCHED } else { 0 };
    self.values.insert(key, (start as u32, len));
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
        dependencies.len() > changes.len() * (dependencies.len().max(1).ilog2() as usize + 1)
      }) {
      self
        .changes
        .iter()
        .rev()
        .take_while(|(epoch, _)| *epoch > since)
        .all(|(_, changes)| {
          changes.iter().all(
            |id| match dependencies.binary_search_by_key(id, |(id, _)| *id) {
              Ok(index) => self.valid(&dependencies[index..index + 1]),
              Err(_) => true,
            },
          )
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
