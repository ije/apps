use super::Build;
use super::create::create;
use super::kept::Read;
use super::query::passed;
use renderer::Child;

pub(crate) fn same_in_place(fresh: &Child, placed: &[renderer::Node], tree: &renderer::Tree) {
  fn find(child: &Child, id: renderer::NodeKey) -> Option<&renderer::Node> {
    let Child::Node(node) = child else {
      return None;
    };
    if node.element_id() == Some(id) {
      return Some(node);
    }
    node.kids().iter().find_map(|kid| find(kid, id))
  }
  fn same(fresh: &renderer::Node, placed: &renderer::Node, tree: &renderer::Tree) {
    let id = fresh.element_id();
    assert!(
      fresh.style_ref() == placed.style_ref(),
      "box {id:?} built in place has another style than a full build gives"
    );
    assert!(
      fresh.text_content() == placed.text_content(),
      "box {id:?} built in place has other text than a full build gives"
    );
    assert_eq!(
      fresh.kids().len(),
      placed.kids().len(),
      "box {id:?} built in place has other children than a full build gives"
    );
    for (fresh, placed) in fresh.kids().iter().zip(placed.kids()) {
      match (fresh, placed) {
        (Child::Node(fresh), Child::Node(placed)) => same(fresh, placed, tree),
        (Child::Node(fresh), Child::Kept(kept) | Child::Placed(kept, _)) => assert!(
          tree.style_of(*kept) == Some(fresh.style_ref()),
          "box {kept:?} kept in a box built in place has another style than a full build gives"
        ),
        _ => {}
      }
    }
  }
  for node in placed {
    if let Some(id) = node.element_id()
      && let Some(fresh) = find(fresh, id)
    {
      same(fresh, node, tree);
    }
  }
}

pub(super) static TRACE: std::sync::LazyLock<bool> =
  std::sync::LazyLock::new(|| std::env::var_os("APPS_TRACE_BUILDS").is_some());

pub(super) fn check(
  build: &Build<'_>,
  id: usize,
  containing: [f32; 2],
  container: [f32; 2],
  slot: Option<([f32; 4], super::super::layout::grid::Grid)>,
  mark: usize,
  printed: usize,
) {
  let culls = std::cell::Cell::new(0);
  let shadow = Build {
    ancestors: build.ancestors.clone(),
    perspective: build.perspective.clone(),
    zone: None,
    culls: &culls,
    checking: true,
    extra: Default::default(),
    plan: Default::default(),
    ..*build
  };
  let start = build.reads.borrow().len();
  let fresh = create(&shadow, id, containing, container, slot, None);
  let depth = build.ancestors.len();
  let set = |reads: &[Read]| {
    let mut set: Vec<_> = reads
      .iter()
      .filter(|read| usize::from(read.scope) <= depth)
      .map(|read| (read.query, usize::from(read.reader) > depth, read.value))
      .collect();
    set.sort_unstable_by_key(|&(query, below, _)| (query, below));
    set.dedup_by_key(|&mut (query, below, _)| (query, below));
    set
  };
  let mut log = build.reads.borrow_mut();
  // A kept box passes its reads on as its parent holds them.
  for read in &mut log[start..] {
    read.query = passed(
      build.styles,
      id,
      read.query,
      usize::from(read.reader) > depth,
    );
  }
  let (kept, now) = (set(&log[mark..start]), set(&log[start..]));
  log.truncate(start);
  drop(log);
  let tag = &build.doc.nodes[id].tag;
  assert!(
    kept == now,
    "kept box {id} <{tag}> reads otherwise when built again: {:?} instead of {:?}",
    now.iter().map(|read| read.0).collect::<Vec<_>>(),
    kept.iter().map(|read| read.0).collect::<Vec<_>>()
  );
  let print = print(&shadow, printed + 1, &fresh);
  let mut prints = build.prints.borrow_mut();
  assert_eq!(
    prints[printed], print,
    "kept box {id} <{tag}> builds otherwise when built again"
  );
  prints.truncate(printed + 1);
}

pub(super) fn print(build: &Build<'_>, from: usize, child: &Child) -> u64 {
  use std::hash::Hasher;
  struct Hashed(rustc_hash::FxHasher);
  impl std::fmt::Write for Hashed {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
      self.0.write(text.as_bytes());
      Ok(())
    }
  }
  let mut hasher = Hashed(Default::default());
  if let Child::Node(node) = child {
    let _ = std::fmt::Write::write_fmt(&mut hasher, format_args!("{:?}", node.style_ref()));
  }
  hasher.0.write_u64(build.extra.take());
  let mut prints = build.prints.borrow_mut();
  for print in prints.drain(from..) {
    hasher.0.write_u64(print);
  }
  let print = hasher.0.finish();
  prints.push(print);
  print
}
