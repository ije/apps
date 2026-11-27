use crate::css::{
  properties::{INSETS, MARGIN, PADDING, Prop, PropSet, Style, Vars},
  value::{anchor, units::Lengths},
};
use crate::dom::{Document, Geometry};
use crate::render::{
  PageState, effects::establishes, element::query::positioner, layout::cull, viewport_overflow,
};
use crate::style::{Chunked, ComputedStyles, Used};
use renderer::{Align, AlignOverflow, Aligned};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const SIZES: [[Prop; 3]; 2] = [
  [Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH],
  [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT],
];
// Like Chrome, a box tries no more options.
const MOST_OPTIONS: usize = 5;
const MOST_PASSES: u32 = 60;

// The properties that decide which anchors a box can have.
const STRUCTURE: PropSet = PropSet::new(&[
  Prop::POSITION,
  Prop::DISPLAY,
  Prop::VISIBILITY,
  Prop::ANCHOR_NAME,
  Prop::ANCHOR_SCOPE,
  Prop::OVERFLOW_X,
  Prop::OVERFLOW_Y,
  Prop::TRANSFORM,
  Prop::TRANSLATE,
  Prop::ROTATE,
  Prop::SCALE,
  Prop::PERSPECTIVE,
  Prop::FILTER,
  Prop::WEBKIT_FILTER,
  Prop::CLIP_PATH,
  Prop::WEBKIT_CLIP_PATH,
  Prop::MASK_IMAGE,
  Prop::WEBKIT_MASK_IMAGE,
  Prop::BACKDROP_FILTER,
  Prop::WEBKIT_BACKDROP_FILTER,
  Prop::MIX_BLEND_MODE,
  Prop::ISOLATION,
  Prop::CONTAIN,
  Prop::CONTENT_VISIBILITY,
  Prop::CONTAINER_TYPE,
  Prop::WILL_CHANGE,
  Prop::MACOS_GLASS,
  Prop::BORDER_TOP_WIDTH,
  Prop::BORDER_RIGHT_WIDTH,
  Prop::BORDER_BOTTOM_WIDTH,
  Prop::BORDER_LEFT_WIDTH,
  Prop::BORDER_TOP_STYLE,
  Prop::BORDER_RIGHT_STYLE,
  Prop::BORDER_BOTTOM_STYLE,
  Prop::BORDER_LEFT_STYLE,
]);

#[derive(Default)]
pub(crate) struct Anchors {
  /// The implicit anchors of popovers: the elements that opened them.
  pub(crate) sources: FxHashMap<usize, usize>,
  /// Counts the changes of `sources`.
  pub(crate) opened: u64,
  pub(crate) boxes: FxHashMap<usize, Box<Anchored>>,
  // The time that the last pass showed.
  pub(super) now: Option<std::time::Instant>,
  // The passes in which boxes moved, in the last frame.
  passes: u32,
  // What the last update read, when it changed nothing.
  read: Option<Read>,
  graph: Graph,
}

#[derive(Clone, PartialEq)]
struct Read {
  epoch: u64,
  layouts: f32,
  scrolls: u64,
  viewport: [f32; 2],
  live: bool,
}

// Which nodes can be anchors of which boxes. It holds while the nodes on its paths keep their places and styles.
#[derive(Default)]
struct Graph {
  epoch: u32,
  nodes: Option<Rc<crate::style::AnchorNodes>>,
  rules: u64,
  top: Vec<usize>,
  opened: u64,
  values: Chunked<Rc<Style>>,
  serial: u64,
  // The boxes in tree order, the anchors of each name, tree and scope by their hash, the last one first, and the
  // nodes on the paths to the root with their styles.
  boxes: Vec<usize>,
  names: FxHashMap<usize, Vec<usize>>,
  watched: RefCell<FxHashMap<usize, Rc<Style>>>,
}

// The containing blocks between an anchor and the one of a box.
struct Chain {
  // The last box before the containing block, when it is out of flow.
  out: Option<usize>,
  scrollers: Rc<[usize]>,
  top: usize,
  sticky: bool,
}

/// A box with an anchor, as of the last layout.
#[derive(Default)]
pub(crate) struct Anchored {
  /// `position-visibility` hides it.
  pub(crate) hidden: Cell<bool>,
  // It is hidden for other reasons than a clipped anchor.
  veiled: bool,
  // The clips that can hide its anchor: scroll containers with their borders. And the boxes around the anchor that
  // can be hidden themselves.
  clips: Vec<(usize, [f32; 4])>,
  hosts: Vec<usize>,
  target: Option<usize>,
  // The scroll containers between the anchor and the containing block, and the scroll that layout placed the box
  // with, which paint makes up for on these axes.
  scrollers: Rc<[usize]>,
  remembered: [f32; 2],
  axes: [bool; 2],
  // The scroll as of the last frame. Like Chrome, a layout that a script forces takes no newer one.
  seen: Cell<[f32; 2]>,
  base: Option<Rc<Style>>,
  epoch: u32,
  // Its own style and its position options.
  specs: Vec<Rc<Spec>>,
  // The option that the box has now, and the last one it fit in.
  chosen: usize,
  last: Option<usize>,
  // The options that are left to try, when it tries them.
  trying: Option<Vec<usize>>,
  // The size that the margin box had in each option, and the width of its room there.
  sizes: Vec<Option<([f32; 2], f32)>>,
  // What the last step read.
  block: Option<Rc<Block>>,
  targets: Vec<Target>,
  size: Option<[f32; 2]>,
  at: [f32; 2],
  vars: Option<Vars>,
  missed: bool,
  settled: bool,
}

impl Anchored {
  /// How far scrolls moved the anchor since layout placed the box, as of the last frame or `now`, and whether the
  /// box is hidden.
  pub(crate) fn prepaint(&self, dom: &PageState, now: bool) -> ([f32; 2], bool) {
    if now && !self.scrollers.is_empty() {
      self
        .seen
        .set(painted(&dom.geometry, &self.scrollers, dom.pixel_ratio));
    }
    if now {
      self.hidden.set(self.hides(dom));
    }
    let seen = self.seen.get();
    let shift = [0, 1].map(|axis| match self.axes[axis] {
      true => seen[axis] - self.remembered[axis],
      false => 0.,
    });
    (shift, self.hidden.get())
  }

  fn hides(&self, dom: &PageState) -> bool {
    let geometry = &dom.geometry;
    if self.veiled {
      return true;
    }
    let boxes = &dom.layout.anchors.boxes;
    if (self.hosts.iter()).any(|id| boxes.get(id).is_some_and(|host| host.hidden.get())) {
      return true;
    }
    let Some(&[x, y, width, height]) = self
      .target
      .filter(|_| !self.clips.is_empty())
      .and_then(|target| geometry.mirror.get(&target))
    else {
      return false;
    };
    let mut view = [x, y, x + width, y + height];
    for (id, [top, right, bottom, left]) in &self.clips {
      let page = [0., 0., *right, *bottom];
      let (rect, [top, right, bottom, left]) = match id {
        0 => (Some(&page), [0.; 4]),
        id => (geometry.mirror.get(id), [*top, *right, *bottom, *left]),
      };
      if let Some(&[x, y, width, height]) = rect {
        view = [
          view[0].max(x + left),
          view[1].max(y + top),
          view[2].min(x + width - right),
          view[3].min(y + height - bottom),
        ];
      }
    }
    view[0] >= view[2] - 0.01 || view[1] >= view[3] - 0.01
  }

  /// Whether the box `id` is in a hidden one.
  pub(crate) fn hidden_around(dom: &PageState, doc: &Document, id: usize) -> bool {
    let Some(styles) = dom
      .styles
      .last()
      .filter(|_| !dom.layout.anchors.boxes.is_empty())
    else {
      return false;
    };
    let parent = |&at: &usize| dom.tree.flat_parent(doc, &styles.children, at);
    std::iter::successors(Some(id), parent)
      .skip(1)
      .any(|at| (dom.layout.anchors.boxes.get(&at)).is_some_and(|found| found.hidden.get()))
  }

  /// Scrolls move or hide the box.
  pub(crate) fn scrolls(&self) -> bool {
    !self.scrollers.is_empty() || !self.hosts.is_empty() || !self.clips.is_empty()
  }
}

// The scroll offsets of `scrollers`, as this prepaint gives them.
fn painted(geometry: &Geometry, scrollers: &[usize], scale: f32) -> [f32; 2] {
  scrollers.iter().fold([0.; 2], |[x, y], id| {
    let at = geometry
      .scroll
      .get(id)
      .map_or_else(Default::default, |scroll| {
        crate::dom::geometry::painted_offset(scroll, scale)
      });
    [x + at.x.0, y + at.y.0]
  })
}

// A containing block: its element, if it is not the viewport, where its padding box is in the window, with its own
// scroll, how much transforms scale it, its size, the size with the content that it scrolls to, if it scrolls, and
// the size of the page for the viewport.
#[derive(PartialEq)]
struct Block {
  id: Option<usize>,
  origin: [f32; 2],
  scale: [f32; 2],
  size: [f32; 2],
  scrolled: Option<[f32; 2]>,
  page: Option<[f32; 2]>,
  rtl: bool,
}

impl Block {
  // Only its place differs.
  fn same(&self, other: &Self) -> bool {
    (self.id, self.scale, self.size, self.rtl) == (other.id, other.scale, other.size, other.rtl)
      && (self.scrolled, self.page) == (other.scrolled, other.page)
  }
}

// An anchor: its box in the containing block without the scrolls between them.
#[derive(Clone)]
struct Target {
  node: usize,
  at: [f32; 2],
  size: [f32; 2],
  chain: Rc<Chain>,
}

#[derive(Clone)]
enum Len {
  Auto,
  Px(f32),
  // Percentages.
  Math(Rc<str>),
  // Anchor functions.
  Anchor(Rc<str>),
  Word(Rc<str>),
}

impl Len {
  fn of(style: &Style, prop: Prop, units: &Lengths) -> Self {
    let Some(value) = style
      .value(prop)
      .filter(|value| !matches!(&***value, "auto" | "none"))
    else {
      return Self::Auto;
    };
    if anchor::has_function(value) {
      return Self::Anchor(value.clone());
    }
    match units.resolve(value, 0.) {
      Some(_) if value.contains('%') => Self::Math(value.clone()),
      Some(length) => Self::Px(length),
      None => Self::Word(value.clone()),
    }
  }

  fn fills(&self) -> bool {
    matches!(self, Self::Word(word) if matches!(&**word, "stretch" | "-webkit-fill-available"))
  }
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
  Normal,
  Start,
  End,
  OwnStart,
  OwnEnd,
  Left,
  Right,
  Center,
  Anchor,
  Stretch,
}

// What places a box in one of its position options.
struct Spec {
  units: Lengths,
  // The style of an option.
  own: Option<Rc<Style>>,
  name: Rc<str>,
  area: Option<Rc<str>>,
  insets: [Len; 4],
  sizes: [[Len; 3]; 2],
  margins: [Len; 4],
  padding: [Len; 4],
  borders: [f32; 2],
  aligns: [(Side, AlignOverflow); 2],
  rtl: bool,
  // A ratio gives an auto size from the other one, and replaced elements have sizes of their own.
  sized: bool,
  // `position-visibility`: `always`, `anchor-valid`, `anchor-visible` and `no-overflow`.
  visibility: [bool; 3],
  order: Option<usize>,
}

impl Spec {
  fn of(style: &Style, units: &Lengths, tag: &str) -> Self {
    let get = |prop: Prop| style.get(prop);
    let name = style.value(Prop::POSITION_ANCHOR);
    let rules = get(Prop::POSITION_VISIBILITY).unwrap_or("anchor-visible");
    let aligns = [Prop::JUSTIFY_SELF, Prop::ALIGN_SELF].map(|prop| {
      let value = get(prop).unwrap_or("auto");
      let (overflow, value) = match value.split_once(' ') {
        Some(("safe", value)) => (AlignOverflow::Safe, value),
        Some(("unsafe", value)) => (AlignOverflow::Unsafe, value),
        _ => (AlignOverflow::Shift, value),
      };
      let side = match value {
        "auto" | "normal" => Side::Normal,
        "end" | "flex-end" | "last baseline" => Side::End,
        "self-start" => Side::OwnStart,
        "self-end" => Side::OwnEnd,
        "left" => Side::Left,
        "right" => Side::Right,
        "center" => Side::Center,
        "anchor-center" => Side::Anchor,
        "stretch" => Side::Stretch,
        _ => Side::Start,
      };
      (side, overflow)
    });
    let len = |prop: Prop| Len::of(style, prop, units);
    Self {
      units: *units,
      own: None,
      name: name.cloned().unwrap_or_else(|| "normal".into()),
      area: (style.value(Prop::POSITION_AREA))
        .filter(|area| &***area != "none")
        .cloned(),
      insets: INSETS.map(len),
      sizes: SIZES.map(|props| props.map(len)),
      margins: MARGIN.map(|prop| match style.contains(prop) {
        true => len(prop),
        false => Len::Px(0.),
      }),
      padding: PADDING.map(len),
      borders: {
        let [top, right, bottom, left] = units.borders(style, 0.);
        [left + right, top + bottom]
      },
      aligns,
      rtl: rtl(style),
      sized: cull::replaced(tag)
        || matches!(
          tag,
          "img" | "input" | "select" | "textarea" | "progress" | "meter"
        ),
      visibility: [
        rules.contains("anchor-valid"),
        rules.contains("anchor-visible"),
        rules.contains("no-overflow"),
      ],
      order: match get(Prop::POSITION_TRY_ORDER) {
        Some("most-width" | "most-inline-size") => Some(0),
        Some("most-height" | "most-block-size") => Some(1),
        _ => None,
      },
    }
    .ratio(style)
  }

  // A size that a ratio gives is `auto`.
  fn ratio(mut self, style: &Style) -> Self {
    let definite = |len: &Len| !matches!(len, Len::Auto);
    if style
      .get(Prop::ASPECT_RATIO)
      .is_some_and(|ratio| ratio != "auto")
      && (definite(&self.sizes[0][0]) || definite(&self.sizes[1][0]))
    {
      self.sized = true;
    }
    self
  }
}

struct Cx<'a> {
  doc: &'a Document,
  dom: &'a PageState,
  styles: &'a ComputedStyles,
  graph: &'a Graph,
  viewport: [f32; 2],
  live: bool,
  // By their element, and by a number of no element for the viewport.
  blocks: RefCell<FxHashMap<usize, Option<Rc<Block>>>>,
  // The anchors that the current box read, and the ones that its last step read.
  reads: RefCell<Vec<Target>>,
  old: RefCell<Vec<Target>>,
  // What its names found.
  found: RefCell<Vec<(usize, Option<Target>)>>,
  // An anchor of the current box doesn't show, so it can come.
  missed: Cell<bool>,
}

// The used position of a box in one option.
struct Placed {
  used: Used,
  scrollers: Rc<[usize]>,
  // The room on each axis, where the box has one, and the margins and edges in it.
  room: [Option<[f32; 2]>; 2],
  outer: [f32; 2],
  target: Option<usize>,
}

fn rtl(style: &Style) -> bool {
  style.get(Prop::DIRECTION) == Some("rtl")
}

impl Cx<'_> {
  fn parent(&self, id: usize) -> Option<usize> {
    (self.dom.tree).flat_parent(self.doc, &self.styles.children, id)
  }

  fn up(&self, id: usize) -> impl Iterator<Item = usize> {
    std::iter::successors(Some(id), |&at| self.parent(at))
  }

  fn root(&self, id: usize) -> usize {
    let up = std::iter::successors(Some(id), |&at| self.doc.nodes[at].parent);
    up.last().unwrap_or(0)
  }

  // The hash of an anchor name with the tree and the scope that it has at `id`: the element of that tree whose
  // `anchor-scope` holds it there.
  fn key(&self, id: usize, name: &str) -> usize {
    use std::hash::BuildHasher;
    let root = self.root(id);
    let scope = self.up(id).find(|&at| {
      (self.styles.values[at].get(Prop::ANCHOR_SCOPE))
        .is_some_and(|list| list == "all" || list.split(',').any(|item| item.trim() == name))
        && self.root(at) == root
    });
    rustc_hash::FxBuildHasher.hash_one((name, root, scope)) as usize
  }

  fn layer(&self, id: usize) -> Option<usize> {
    self.graph.top.iter().position(|&node| node == id)
  }

  fn scrolls(&self, id: usize) -> bool {
    [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
      .into_iter()
      .any(|prop| {
        (self.styles.values[id].get(prop))
          .is_some_and(|value| matches!(value, "auto" | "scroll" | "hidden"))
      })
      && !viewport_overflow(self.doc, self.styles, id)
  }

  fn scroll(&self, scrollers: &[usize]) -> [f32; 2] {
    scrollers.iter().fold([0.; 2], |[x, y], &id| {
      let at = self.dom.geometry.mirror_offset(id);
      [x + at.x.0, y + at.y.0]
    })
  }

  // The containing block of an out-of-flow box: `None` for the viewport, 0 for the initial containing block.
  fn positioner(&self, id: usize) -> Option<usize> {
    let position = self.styles.values[id]
      .get(Prop::POSITION)
      .unwrap_or("static");
    let top = self.layer(id).is_some();
    let up = self.up(id).skip(1).filter(|_| !top);
    positioner(position, up, |at| establishes(&self.styles.values[at]))
  }

  fn block(&self, found: Option<usize>) -> Option<Rc<Block>> {
    if let Some(block) = self.blocks.borrow().get(&found.unwrap_or(usize::MAX)) {
      return block.clone();
    }
    let block = self.measure(found).map(Rc::new);
    let key = found.unwrap_or(usize::MAX);
    self.blocks.borrow_mut().insert(key, block.clone());
    block
  }

  fn measure(&self, found: Option<usize>) -> Option<Block> {
    let geometry = &self.dom.geometry;
    let Some(node) = found.filter(|&at| at != 0) else {
      let html = self.doc.nodes[0].children.iter().copied();
      let start = geometry.mirror_offset(0);
      let start = [start.x.0, start.y.0];
      let [bottom, right] = geometry.page_bars;
      let size = [self.viewport[0] - right, self.viewport[1] - bottom];
      let page = self.extent(html.clone(), start, size);
      return Some(Block {
        id: found,
        origin: found.map_or([0.; 2], |_| start),
        scale: [1.; 2],
        size,
        scrolled: found.map(|_| page),
        page: found.is_none().then_some(page),
        rtl: html
          .last()
          .is_some_and(|html| rtl(&self.styles.values[html])),
      });
    };
    let rect = geometry.mirror.get(&node).copied()?;
    let style = &self.styles.values[node];
    let units = &self.styles.lengths[node];
    let [top, right, bottom, left] = units.borders(style, rect[2]);
    let size = self
      .dom
      .layout
      .blocks
      .get(&node)
      .map_or([rect[2] - left - right, rect[3] - top - bottom], |block| {
        [block.size.width.0, block.size.height.0]
      });
    let scale = [
      rect[2] / (size[0] + left + right).max(0.01),
      rect[3] / (size[1] + top + bottom).max(0.01),
    ]
    .map(|scale| {
      if (scale - 1.).abs() < 0.001 {
        1.
      } else {
        scale
      }
    });
    let at = geometry.mirror_offset(node);
    let origin = [
      rect[0] + (left + at.x.0) * scale[0],
      rect[1] + (top + at.y.0) * scale[1],
    ];
    Some(Block {
      id: found,
      origin,
      scale,
      size,
      scrolled: self.scrolls(node).then(|| {
        let nodes = self.styles.flat_children(self.doc, node).iter().copied();
        let far = self.extent(nodes, origin, [0.; 2]);
        let ends = [Prop::PADDING_RIGHT, Prop::PADDING_BOTTOM].map(|prop| {
          (style.get(prop))
            .and_then(|value| units.resolve(value, rect[2]))
            .unwrap_or(0.)
        });
        [0, 1].map(|axis| size[axis].max(far[axis] / scale[axis] + ends[axis]))
      }),
      page: None,
      rtl: rtl(style),
    })
  }

  // How far in-flow boxes reach from `origin`, where `size` is the least.
  fn extent(
    &self,
    nodes: impl Iterator<Item = usize>,
    origin: [f32; 2],
    size: [f32; 2],
  ) -> [f32; 2] {
    let mut far = size;
    for node in nodes {
      let style = &self.styles.values[node];
      let Some(&[x, y, width, height]) = self.dom.geometry.mirror.get(&node) else {
        continue;
      };
      let position = style.get(Prop::POSITION).unwrap_or("static");
      if matches!(position, "absolute" | "fixed") || !self.dom.layout.shown(node) {
        continue;
      }
      let units = &self.styles.lengths[node];
      let length = |prop: Prop| style.get(prop).and_then(|value| units.resolve(value, 0.));
      // A relative offset moves the box but not its place in the flow.
      let offset = |start: Prop, end: Prop| match position {
        "relative" => length(start)
          .or_else(|| length(end).map(|end| -end))
          .unwrap_or(0.),
        _ => 0.,
      };
      let ends = [
        x + width - offset(Prop::LEFT, Prop::RIGHT) + length(Prop::MARGIN_RIGHT).unwrap_or(0.),
        y + height - offset(Prop::TOP, Prop::BOTTOM) + length(Prop::MARGIN_BOTTOM).unwrap_or(0.),
      ];
      far = [0, 1].map(|axis| far[axis].max(ends[axis] - origin[axis]));
    }
    far
  }

  // The containing blocks from the anchor `node` to `block`, if it is in the block.
  fn walk(&self, node: usize, block: Option<usize>) -> Option<Chain> {
    let mut scrollers = Vec::new();
    // The last box on the chain, and how it looks for the next one.
    let (mut inner, mut seek, mut sticky) = (node, "", false);
    let mut at = Some(node);
    let found = loop {
      let Some(next) = at else {
        break block.is_none_or(|outer| outer == 0 && seek != "fixed");
      };
      let style = &self.styles.values[next];
      // The shapes of an SVG image are no boxes.
      if next != node && self.doc.nodes[next].tag == "svg"
        || style.get(Prop::DISPLAY) == Some("none")
      {
        return None;
      }
      let chain = match seek {
        "" => true,
        "fixed" => establishes(style).fixed,
        _ => establishes(style).absolute,
      };
      if chain && next != 0 {
        if block == Some(next) && next != node {
          break true;
        }
        if next != node && self.scrolls(next) {
          scrollers.push(next);
        }
        inner = next;
        let position = style.get(Prop::POSITION).unwrap_or("static");
        sticky |= position == "sticky";
        seek = match position {
          "absolute" | "fixed" => position,
          _ => "",
        };
      }
      if self.layer(next).is_some() {
        break match block {
          Some(0) => seek == "absolute",
          block => block.is_none(),
        };
      }
      at = self.parent(next);
    };
    if !found {
      return None;
    }
    if block.is_none() && seek != "fixed" && self.layer(inner).is_none() {
      scrollers.push(0);
    }
    Some(Chain {
      out: (!seek.is_empty()).then_some(inner),
      scrollers: scrollers.into(),
      top: self
        .up(node)
        .find_map(|at| self.layer(at))
        .map_or(0, |at| at + 1),
      sticky,
    })
  }

  // The anchor `node` for the box `id` in `block`, if it can be one: a box in the containing block that lays out
  // before the box.
  fn target(&self, node: usize, id: usize, block: &Block) -> Option<Target> {
    if let Some(read) = self.reads.borrow().iter().find(|read| read.node == node) {
      return Some(read.clone());
    }
    let chain = Rc::new(self.walk(node, block.id)?);
    match chain.out {
      Some(out) => {
        let order = |id: usize| (self.layer(id), self.dom.styles.walk.place(id));
        let top = |id: usize| self.layer(id).map_or(0, |at| at + 1);
        let (own, other) = (order(id), order(out));
        if top(id) < top(out) || top(id) == top(out) && own.1 <= other.1 {
          return None;
        }
      }
      None if chain.top == 0 => {}
      None => {
        if chain.top
          > self
            .up(id)
            .find_map(|at| self.layer(at))
            .map_or(0, |at| at + 1)
        {
          return None;
        }
      }
    }
    let target = self.locate(node, block, chain)?;
    self.reads.borrow_mut().push(target.clone());
    Some(target)
  }

  // Where the anchor is now.
  fn locate(&self, node: usize, block: &Block, chain: Rc<Chain>) -> Option<Target> {
    let Some(rect) = (self.dom.geometry.mirror.get(&node).copied())
      .filter(|rect| *rect != [0.; 4] && self.dom.layout.shown(node))
    else {
      self.missed.set(true);
      return None;
    };
    let scroll = self.scroll(&chain.scrollers);
    let mut target = Target {
      node,
      at: [0, 1].map(|axis| (rect[axis] - block.origin[axis]) / block.scale[axis] - scroll[axis]),
      size: [0, 1].map(|axis| rect[axis + 2] / block.scale[axis]),
      chain,
    };
    // Sums of scroll offsets add noise, and a sticky anchor moves with scrolls that a forced layout doesn't see.
    if let Some(old) = self.old.borrow().iter().find(|old| old.node == node) {
      for axis in 0..2 {
        if (old.at[axis] - target.at[axis]).abs() < 0.04 {
          target.at[axis] = old.at[axis];
        }
      }
      if !self.live && old.chain.sticky {
        target.at = old.at;
      }
    }
    Some(target)
  }

  fn named(&self, name: &str, id: usize, block: &Block) -> Option<Target> {
    let key = self.key(id, name);
    let found = self
      .found
      .borrow()
      .iter()
      .find(|found| found.0 == key)
      .cloned();
    if let Some((_, target)) = found {
      return target;
    }
    let nodes = self.graph.names.get(&key)?;
    let target = (nodes.iter()).find_map(|&node| self.target(node, id, block));
    self.found.borrow_mut().push((key, target.clone()));
    target
  }

  // The default anchor of a box.
  fn default(&self, id: usize, spec: &Spec, block: &Block) -> Option<Target> {
    match &*spec.name {
      "none" => None,
      "normal" if spec.area.is_none() => None,
      "auto" | "normal" => self.target(*self.dom.layout.anchors.sources.get(&id)?, id, block),
      name => self.named(name, id, block),
    }
  }

  // The style of a position option of a box with the style `base`.
  fn option(&self, id: usize, base: &Rc<Style>, option: &str) -> Option<Style> {
    let mut style = (**base).clone();
    let mut flips = Vec::new();
    for word in option.split(' ') {
      if word.starts_with("--") {
        let parent = self.parent(id).map(|parent| &*self.styles.values[parent]);
        let cache = &self.dom.styles.cache;
        cache.position_try(self.doc, (id, word), &mut style, parent, self.styles)?;
      } else if word.starts_with("flip-") {
        flips.push(word);
      } else {
        style.insert(Prop::POSITION_AREA, option);
        break;
      }
    }
    let mirrored = rtl(&style);
    anchor::flip(&flips, mirrored, &mut style);
    Some(style)
  }

  // The own style of a box and its position options.
  fn specs(&self, id: usize, base: &Rc<Style>) -> Vec<Rc<Spec>> {
    let units = &self.styles.lengths[id];
    let tag = &self.doc.nodes[id].tag;
    let mut specs = vec![Rc::new(Spec::of(base, units, tag))];
    let fallbacks = base.get(Prop::POSITION_TRY_FALLBACKS).unwrap_or("none");
    // A value with `var()` is as written.
    let fallbacks = anchor::specified("position-try-fallbacks", fallbacks);
    let fallbacks = fallbacks.as_deref().unwrap_or("none");
    for option in fallbacks.split(", ").filter(|option| *option != "none") {
      if specs.len() <= MOST_OPTIONS
        && let Some(style) = self.option(id, base, option)
      {
        let mut spec = Spec::of(&style, units, tag);
        spec.own = Some(Rc::new(style));
        specs.push(Rc::new(spec));
      }
    }
    specs
  }

  // Whether the anchors and the containing block are where the last step found them.
  fn still(&self, state: &Anchored, block: &Rc<Block>) -> bool {
    let Some(old) = &state.block else {
      return false;
    };
    old.same(block)
      && state.targets.iter().all(|old| {
        let rect = self.dom.geometry.mirror.get(&old.node);
        let scroll = self.scroll(&old.chain.scrollers);
        self.dom.layout.shown(old.node)
          && (!self.live && old.chain.sticky
            || rect.is_some_and(|rect| {
              (0..2).all(|axis| {
                let at = (rect[axis] - block.origin[axis]) / block.scale[axis];
                (at - scroll[axis] - old.at[axis]).abs() < 0.04
                  && (rect[axis + 2] / block.scale[axis] - old.size[axis]).abs() < 0.04
              })
            }))
      })
  }
}

// The position of a box, where scrolls moved its anchor by `seen`. Without a containing block, it has no anchors.
fn place(context: Option<(&Cx, usize, &Block)>, spec: &Spec, seen: [f32; 2]) -> Box<Placed> {
  let units = &spec.units;
  let default = context.and_then(|(cx, id, block)| cx.default(id, spec, block));
  let default = default.as_ref();
  let block = context.map(|context| context.2);
  let direction = [block.is_some_and(|block| block.rtl), spec.rtl];
  // A box with an anchor takes all that its containing block scrolls to as containing block.
  let client = block.map_or([0.; 2], |block| block.size);
  let wide = block.and_then(|block| block.scrolled.filter(|_| default.is_some()));
  let size = wide.unwrap_or(client);
  let moves = |target: &Target| {
    default.is_some_and(|default| target.chain.scrollers == default.chain.scrollers)
  };
  // The place of an anchor on an axis.
  let span = |target: &Target, axis: usize| {
    let start = target.at[axis] + if moves(target) { seen[axis] } else { 0. };
    [start, start + target.size[axis]]
  };
  let area = default
    .zip(spec.area.as_deref())
    .and_then(|(target, area)| {
      let tracks = anchor::area(area, direction)?;
      let lines = [0, 1].map(|axis| {
        let [start, end] = span(target, axis);
        let lines = [start.min(0.), start, end, end.max(size[axis])];
        tracks[axis].map(|line| lines[usize::from(line)])
      });
      Some((tracks, lines))
    });
  let frame = area.map_or([[0., size[0]], [0., size[1]]], |(_, lines)| lines);
  let extent = [0, 1].map(|axis| frame[axis][1] - frame[axis][0]);
  let mut axes = [area.is_some(); 2];
  // A value with the functions resolved, or `None` when it is invalid. `side` is the one of an inset.
  let mut function = |function: &anchor::Function, side: Option<usize>, axis: usize| {
    let (cx, id, block) = context?;
    let named;
    let target = match function.name {
      Some(name) => {
        named = cx.named(name, id, block)?;
        &named
      }
      None => default?,
    };
    if function.size {
      let axis = match &*function.side {
        "width" | "inline" | "self-inline" => 0,
        "height" | "block" | "self-block" => 1,
        _ => axis,
      };
      return Some(target.size[axis]);
    }
    let far = matches!(side?, 1 | 2);
    let turn = |part: f32, turned: bool| if turned { 1. - part } else { part };
    let turned = |own: usize| axis == 0 && direction[own];
    let part = match (&*function.side, axis) {
      ("left", 0) | ("top", 1) => 0.,
      ("right", 0) | ("bottom", 1) => 1.,
      ("inside", _) => turn(0., far),
      ("outside", _) => turn(1., far),
      ("start", _) => turn(0., turned(0)),
      ("end", _) => turn(1., turned(0)),
      ("self-start", _) => turn(0., turned(1)),
      ("self-end", _) => turn(1., turned(1)),
      ("center", _) => 0.5,
      (percent, _) => turn(units.resolve(percent, 1.)?, turned(0)),
    };
    axes[axis] |= moves(target);
    let [start, end] = span(target, axis);
    let at = start + part * (end - start);
    Some(if far {
      frame[axis][1] - at
    } else {
      at - frame[axis][0]
    })
  };
  // The length, and whether the anchors changed it. Percentages are of the frame when that is not the box that
  // layout takes them from.
  let mut resolve = |len: &Len, side: Option<usize>, axis: usize, base: usize| match len {
    Len::Auto | Len::Word(_) => (None, false),
    Len::Px(length) => (Some(*length), false),
    Len::Math(text) => (
      units.resolve(text, extent[base]),
      area.is_some() || wide.is_some(),
    ),
    Len::Anchor(text) => {
      let zoom = units.font.zoom;
      let text = resolved(text, &mut |found| {
        function(found, side, axis).map(|value| value / zoom)
      });
      (
        text.and_then(|text| units.resolve(&text, extent[base])),
        true,
      )
    }
  };
  let mut used = Used {
    own: spec.own.clone(),
    ..Used::default()
  };
  let mut insets = [None; 4];
  for (side, len) in spec.insets.iter().enumerate() {
    let axis = 1 - side % 2;
    let (inset, changed) = resolve(len, Some(side), axis, axis);
    insets[side] = inset;
    let far = matches!(side, 1 | 2);
    if changed || wide.is_some() && far && inset.is_some() {
      used.shown[side] = Some(inset);
      let moved = if far { client[axis] - size[axis] } else { 0. };
      used.insets[side] = Some(inset.map(|inset| inset + moved));
    }
  }
  for (axis, lens) in spec.sizes.iter().enumerate() {
    for (len, prop) in lens.iter().zip(SIZES[axis]) {
      let (length, changed) = resolve(len, None, axis, axis);
      if changed {
        used
          .lengths
          .push((prop, length.map(|length| length.max(0.))));
      }
    }
  }
  let mut margins = [0.; 4];
  for (side, len) in spec.margins.iter().enumerate() {
    let (length, changed) = resolve(len, None, 1 - side % 2, 0);
    // Auto margins of a box that an anchor aligns are 0.
    let auto = matches!(len, Len::Auto)
      && (area.is_some() || default.is_some() && spec.aligns[1 - side % 2].0 == Side::Anchor);
    margins[side] = length.unwrap_or(0.);
    if changed || auto {
      used.lengths.push((MARGIN[side], length));
    }
  }
  let mut padding = [0.; 4];
  for (side, len) in spec.padding.iter().enumerate() {
    let (length, changed) = resolve(len, None, 0, 0);
    padding[side] = length.unwrap_or(0.);
    if changed {
      used.lengths.push((PADDING[side], length));
    }
  }
  let mut room = [None; 2];
  for axis in 0..2 {
    let [first, last] = [[3, 1], [0, 2]][axis];
    let (start, end) = (insets[first], insets[last]);
    let (side, overflow) = spec.aligns[axis];
    let centered = side == Side::Anchor && default.is_some();
    let Some(block) = block.filter(|_| area.is_some() || centered) else {
      if block.is_some() && (start.is_some() || end.is_some()) {
        room[axis] = Some([start.unwrap_or(0.), size[axis] - end.unwrap_or(0.)]);
      }
      continue;
    };
    let from = frame[axis][0] + start.unwrap_or(0.);
    let to = frame[axis][1] - end.unwrap_or(0.);
    room[axis] = Some([from, to]);
    let turned = |own: usize| axis == 0 && direction[own];
    let flip = |start: bool, turned: bool| match start != turned {
      true => Align::Start,
      false => Align::End,
    };
    let center = default.map_or(Align::Center, |target| {
      let [start, end] = span(target, axis);
      Align::At((start + end) / 2.)
    });
    let align = match side {
      Side::Normal if start.is_some() != end.is_some() => flip(start.is_some(), false),
      Side::Normal => match area.map(|(tracks, _)| tracks[axis]) {
        Some([0, 3]) => center,
        Some([1, 2]) => Align::Center,
        Some([0, _]) => Align::End,
        _ => Align::Start,
      },
      Side::Start => flip(true, turned(0)),
      Side::End => flip(false, turned(0)),
      Side::OwnStart => flip(true, turned(1)),
      Side::OwnEnd => flip(false, turned(1)),
      Side::Right => flip(axis == 1, false),
      Side::Center => Align::Center,
      Side::Anchor => center,
      Side::Left | Side::Stretch => Align::Start,
    };
    axes[axis] |= centered;
    used.place[axis] = Some(Aligned {
      room: [from, to],
      align,
      overflow,
      rtl: turned(0),
      // Like Chrome, a fixed box can overflow the viewport where the page scrolls.
      limit: match wide {
        None => block.page.map_or(0., |page| page[axis]),
        Some(_) => f32::INFINITY,
      },
    });
    for (index, value) in [(first, from), (last, client[axis] - to)] {
      used.insets[index] = Some(Some(value));
      used.shown[index] = Some(Some(insets[index].unwrap_or(0.)));
    }
    let len = &spec.sizes[axis][0];
    used.stretch[axis] = Some(side == Side::Stretch || len.fills());
    used.fit[axis] = matches!(len, Len::Auto) && side != Side::Stretch && !spec.sized;
    if len.fills() {
      used.lengths.push((SIZES[axis][0], None));
    }
  }
  used.anchored = block.is_some();
  let sides = |sides: [f32; 4]| [sides[1] + sides[3], sides[0] + sides[2]];
  let (margins, padding) = (sides(margins), sides(padding));
  Box::new(Placed {
    scrollers: default.map_or_else(
      || Rc::new([]) as Rc<[usize]>,
      |target| target.chain.scrollers.clone(),
    ),
    room,
    outer: [0, 1].map(|axis| margins[axis] + padding[axis] + spec.borders[axis]),
    target: default.map(|target| target.node),
    used: Used {
      axes,
      ..used.unzoomed(units.font.zoom)
    },
  })
}

fn resolved(
  text: &str,
  function: &mut dyn FnMut(&anchor::Function) -> Option<f32>,
) -> Option<String> {
  anchor::replace(text, &mut |found| match function(found) {
    Some(value) => Some(format!("{value}px")),
    None => resolved(found.fallback?, function),
  })
}

/// The values of a box that layout didn't place, for `getComputedStyle()`.
pub(crate) fn unplaced(styles: &ComputedStyles, id: usize) -> Option<Used> {
  let boxed = (styles.anchors.as_ref()).is_some_and(|nodes| nodes.boxes.contains(&id));
  let spec = |style: &Style| Spec::of(style, &styles.lengths[id], "");
  boxed.then(|| place(None, &spec(styles.unanchored(id)), [0.; 2]).used)
}

impl Graph {
  // Whether the nodes that decide about anchors are as they were.
  fn holds(&self, dom: &PageState, styles: &ComputedStyles) -> bool {
    let same = match (&self.nodes, &styles.anchors) {
      (Some(old), Some(new)) => Rc::ptr_eq(old, new) || old == new,
      _ => false,
    };
    let (walk, watched) = (&dom.styles.walk, self.watched.borrow());
    same
      && self.rules == dom.styles.cache.rules
      && self.top == dom.interaction.top_layer
      && self.opened == dom.layout.anchors.opened
      && (walk.serial == self.serial
        || (walk.moved_since(self.serial))
          .is_some_and(|moved| !moved.iter().any(|&id| watched.contains_key(&(id as usize)))))
      && (styles.values.differing(&self.values).flatten()).all(|id| {
        (self.values.get(id)).is_some_and(|old| Rc::ptr_eq(old, &styles.values[id]))
          || (watched.get(&id)).is_none_or(|old| {
            let new = styles.unanchored(id);
            Rc::ptr_eq(old, new) || !old.differs_in(new, &STRUCTURE)
          })
      })
  }

  // The path from a node to the root decides about its anchors.
  fn watch(&self, cx: &Cx, id: usize) {
    let mut watched = self.watched.borrow_mut();
    for at in cx.up(id) {
      if watched
        .insert(at, cx.styles.unanchored(at).clone())
        .is_some()
      {
        break;
      }
    }
  }

  fn rebuild(&mut self, cx: &Cx, nodes: &Rc<crate::style::AnchorNodes>) {
    let sorted = |ids: &rustc_hash::FxHashSet<usize>| {
      let place = |&id: &usize| Some((cx.dom.styles.walk.place(id)? as usize) << 32 | id);
      let mut list: Vec<usize> = ids.iter().filter_map(place).collect();
      list.sort_unstable();
      list.into_iter().map(|at| at & u32::MAX as usize)
    };
    self.boxes = sorted(&nodes.boxes).collect();
    let mut names: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
    for id in sorted(&nodes.names).rev() {
      let list = cx.styles.values[id].get(Prop::ANCHOR_NAME).unwrap_or("");
      for name in list.split(',') {
        names.entry(cx.key(id, name.trim())).or_default().push(id);
      }
    }
    self.names = names;
    self.watched.take();
    for &id in &nodes.names {
      self.watch(cx, id);
    }
  }
}

/// Places the boxes with anchors by the layout before. `first` is the first pass of a frame, or of a layout that a
/// script forces, which is not `live`. True when another pass must follow.
pub(super) fn update(
  doc: &Document,
  dom: &mut PageState,
  viewport: [f32; 2],
  [first, live]: [bool; 2],
) -> bool {
  // A popover that left the top layer in this frame made the styles old.
  let Some(styles) = dom.styles.last().cloned() else {
    return false;
  };
  let Some(nodes) = styles.anchors.clone() else {
    if !dom.layout.anchors.boxes.is_empty() {
      dom.layout.anchors.boxes.clear();
      dom.styles.cache.anchored = Rc::default();
    }
    return false;
  };
  let anchors = &mut dom.layout.anchors;
  if first {
    anchors.passes = 0;
  }
  let read = Read {
    epoch: dom.styles.epoch,
    layouts: dom.geometry.mirror.changed(),
    scrolls: (dom.geometry.mirror_offsets.iter())
      .map(|(&id, at)| {
        use std::hash::BuildHasher;
        let at = [at.x.0.to_bits(), at.y.0.to_bits()];
        rustc_hash::FxBuildHasher.hash_one((id, at))
      })
      .fold(0, u64::wrapping_add),
    viewport,
    live,
  };
  if anchors.read.as_ref() == Some(&read) {
    return false;
  }
  anchors.read = None;
  let mut graph = std::mem::take(&mut anchors.graph);
  let holds = graph.holds(dom, &styles);
  if !holds {
    graph.epoch = graph.epoch.wrapping_add(1);
    graph.top = dom.interaction.top_layer.clone();
    graph.nodes = Some(nodes.clone());
    graph.rules = dom.styles.cache.rules;
    graph.opened = dom.layout.anchors.opened;
  }
  graph.serial = dom.styles.walk.serial;
  graph.values = styles.values.clone();
  let mut states = std::mem::take(&mut dom.layout.anchors.boxes);
  let mut changes: Vec<(usize, Option<Rc<Used>>)> = Vec::new();
  let mut again = false;
  {
    let blank = Graph::default();
    let cx = |graph| Cx {
      doc,
      dom,
      styles: &styles,
      graph,
      viewport,
      live,
      blocks: Default::default(),
      reads: Default::default(),
      old: Default::default(),
      found: Default::default(),
      missed: Default::default(),
    };
    if !holds {
      let mut new = std::mem::take(&mut graph);
      new.rebuild(&cx(&blank), &nodes);
      graph = new;
      states.retain(|id, _| nodes.boxes.contains(id));
    }
    let cx = cx(&graph);
    for &id in &graph.boxes {
      if !dom.layout.shown(id) || dom.geometry.mirror.get(&id).is_none() {
        states.remove(&id);
        continue;
      }
      let base = styles.unanchored(id);
      let state = states.entry(id).or_default();
      if state.epoch != graph.epoch {
        graph.watch(&cx, id);
      }
      let flows =
        !(base.get(Prop::POSITION)).is_some_and(|value| matches!(value, "absolute" | "fixed"));
      if flows {
        // Functions outside out-of-flow boxes give their fallbacks.
        if state.base.as_ref().is_none_or(|old| !Rc::ptr_eq(old, base)) {
          **state = Anchored::default();
          state.base = Some(base.clone());
          let spec = Spec::of(base, &styles.lengths[id], "");
          changes.push((id, Some(Rc::new(place(None, &spec, [0.; 2]).used))));
        }
        continue;
      }
      let found = match state.epoch == graph.epoch && state.block.is_some() {
        true => state.block.as_ref().and_then(|block| block.id),
        false => cx.positioner(id),
      };
      let Some(block) = cx.block(found) else {
        continue;
      };
      let size = dom.layout.content_size(id);
      if state.settled
        && !state.missed
        && (state.vars.as_ref()).is_some_and(|old| Rc::ptr_eq(old, &styles.vars[id]))
        && state.epoch == graph.epoch
        && state.base.as_ref().is_some_and(|old| Rc::ptr_eq(old, base))
        && state.size == size
        && state.specs[0].units == styles.lengths[id]
        && cx.still(state, &block)
        && (state.specs.len() == 1 && !state.specs[0].visibility[2] || state.at == state.seen.get())
      {
        continue;
      }
      let old = dom.styles.cache.anchored.get(&id);
      let (used, more) = step(&cx, id, state, base, &block, size, old);
      again |= more;
      if old.is_none_or(|old| **old != used) {
        state.settled = false;
        changes.push((id, Some(Rc::new(used))));
      } else {
        state.settled = !more;
      }
      state.block = Some(block);
      state.epoch = graph.epoch;
    }
  }
  for id in dom.styles.cache.anchored.keys() {
    if !nodes.boxes.contains(id) {
      changes.push((*id, None));
    }
  }
  let anchors = &mut dom.layout.anchors;
  (anchors.graph, anchors.boxes) = (graph, states);
  if !changes.is_empty() {
    let list = Rc::make_mut(&mut dom.styles.cache.anchored);
    for (id, used) in changes {
      match used {
        Some(used) => list.insert(id, used),
        None => list.remove(&id),
      };
    }
    dom.styles.invalidate();
    again = true;
  }
  // A clip can hide a box whose anchor prepainted after it.
  if live {
    for state in dom.layout.anchors.boxes.values() {
      let hidden = state.hides(dom);
      again |= state.hidden.replace(hidden) != hidden;
    }
  }
  if !again {
    dom.layout.anchors.read = Some(read);
    return false;
  }
  dom.layout.anchors.passes += 1;
  dom.layout.anchors.passes < MOST_PASSES
}

// The position of a box from the last layout, and whether another pass must follow. A box that overflows its room
// tries its position options, one for each pass, and keeps the one that fits until it overflows.
fn step(
  cx: &Cx,
  id: usize,
  state: &mut Anchored,
  base: &Rc<Style>,
  block: &Block,
  inner: Option<[f32; 2]>,
  layout: Option<&Rc<Used>>,
) -> (Used, bool) {
  cx.reads.take();
  cx.found.take();
  *cx.old.borrow_mut() = std::mem::take(&mut state.targets);
  let old = state.base.replace(base.clone());
  let same = old.as_ref().is_some_and(|old| Rc::ptr_eq(old, base))
    && (state.vars.as_ref()).is_some_and(|old| Rc::ptr_eq(old, &cx.styles.vars[id]))
    && state.epoch == cx.graph.epoch
    && (state.specs.first()).is_some_and(|spec| spec.units == cx.styles.lengths[id]);
  if !same {
    let count = state.specs.len();
    state.specs = cx.specs(id, base);
    state.sizes = vec![None; state.specs.len()];
    // A rule came or went.
    if count != state.specs.len() {
      (state.last, state.trying) = (None, None);
    }
  }
  // The order doesn't reset the choice, but other options do.
  let mut fresh = old.is_none_or(|old| {
    [Prop::POSITION_TRY_FALLBACKS, Prop::POSITION]
      .iter()
      .any(|&prop| old.get(prop) != base.get(prop))
  });
  if fresh || state.chosen >= state.specs.len() {
    (state.chosen, state.last, state.trying) = (0, None, None);
  }
  let specs = state.specs.clone();
  let spec = |at: usize| specs[at].clone();
  let at = |spec: &Spec, seen: [f32; 2]| place(Some((cx, id, block)), spec, seen);
  let chosen = spec(state.chosen);
  let found = cx.default(id, &chosen, block);
  let (target, scrollers) = (
    found.as_ref().map(|found| found.node),
    found.map_or_else(Default::default, |found| found.chain.scrollers.clone()),
  );
  let start = fresh;
  // Layout places a box again with the scrolls of another anchor.
  fresh |= state.target != target || state.scrollers != scrollers;
  let before = state.seen.get();
  if cx.live || fresh {
    state.seen.set(cx.scroll(&scrollers));
  }
  let seen = state.seen.get();
  if fresh {
    state.remembered = seen;
  }
  state.target = target;
  let now = at(&chosen, seen);
  // The size of the margin box, as layout gave it with the style of the last pass.
  let size = inner.map(|inner| [0, 1].map(|axis| inner[axis] + now.outer[axis]));
  let width = |placed: &Placed| placed.room[0].map_or(0., |room| room[1] - room[0]);
  let fits = |placed: &Placed, size: [f32; 2]| {
    (0..2).all(|axis| placed.room[axis].is_none_or(|room| size[axis] <= room[1] - room[0] + 0.02))
  };
  let kept = at(&chosen, state.remembered);
  // Whether the scroll moved the box out of its room, where the next layout puts it back.
  let strays = |size: [f32; 2]| {
    (0..2).any(|axis| {
      let moved = match kept.used.axes[axis] {
        true => seen[axis] - state.remembered[axis],
        false => 0.,
      };
      (kept.used.place[axis].zip(now.room[axis])).is_some_and(|(place, room)| {
        let at = place.place(size[axis], block.size[axis]) + moved;
        moved != 0. && (at < room[0] - 0.02 || at + size[axis] > room[1] + 0.02)
      })
    })
  };
  let hides = chosen.visibility[2];
  let mut overflows = false;
  let (mut choose, mut back) = (None, false);
  if specs.len() == 1 {
    if let Some(size) = size.filter(|_| hides) {
      overflows = !fits(&now, size);
      back = overflows || strays(size);
    }
  } else {
    let sorted = || {
      let mut all: Vec<usize> = Vec::new();
      let extent = |option: usize| {
        chosen.order.map_or(0., |axis| {
          let room = at(&spec(option), seen).room[axis];
          room.map_or(f32::NEG_INFINITY, |room| room[1] - room[0])
        })
      };
      for option in 0..specs.len() {
        let wide = extent(option);
        let place = all.iter().position(|&other| extent(other) < wide);
        all.insert(place.unwrap_or(all.len()), option);
      }
      all
    };
    // Layout has the size of this option only when it had the style of this option.
    let current = layout.is_some_and(|layout| {
      let scrolls = layout.scrolls;
      **layout
        == Used {
          scrolls,
          ..kept.used.clone()
        }
    });
    if start && chosen.order.is_some() {
      let mut all = sorted();
      choose = Some(all.remove(0));
      state.trying = Some(all);
    } else if let Some(size) = size {
      overflows = !fits(&now, size);
      if current {
        // Other content gives other sizes in all options.
        if state.sizes[state.chosen].is_some_and(|(old, wide)| {
          (wide - width(&now)).abs() < 0.5
            && ((old[0] - size[0]).abs() > 0.02 || (old[1] - size[1]).abs() > 0.02)
        }) {
          state.sizes.fill(None);
        }
        state.sizes[state.chosen] = Some((size, width(&now)));
        if overflows {
          let mut left = state.trying.take().unwrap_or_else(|| {
            let mut all = sorted();
            all.retain(|&option| option != state.chosen);
            all
          });
          // An option whose size is known can't fit in too little room.
          left.retain(|&option| {
            let placed = at(&spec(option), seen);
            match state.sizes[option] {
              Some((size, wide)) if (wide - width(&placed)).abs() < 0.5 => fits(&placed, size),
              _ => true,
            }
          });
          if left.is_empty() {
            back = true;
            choose = Some(state.last.unwrap_or(0));
          } else {
            choose = Some(left.remove(0));
            state.trying = Some(left);
          }
        } else {
          state.last = Some(state.chosen);
          state.trying = None;
          back = strays(size);
        }
      }
    }
  }
  choose = choose.filter(|&option| option != state.chosen);
  if back {
    state.remembered = seen;
  }
  let chosen = match choose {
    Some(option) => {
      state.chosen = option;
      let chosen = spec(option);
      let placed = at(&chosen, seen);
      if placed.target != state.target || placed.scrollers != scrollers {
        state.target = placed.target;
        state.seen.set(cx.scroll(&placed.scrollers));
      }
      state.remembered = state.seen.get();
      chosen
    }
    None => chosen,
  };
  let placed = at(&chosen, state.remembered);
  let seen = state.seen.get();
  // A box that prepainted before a scroll container moved with its old offset.
  let moved = !fresh
    && state.scrollers == placed.scrollers
    && (0..2).any(|axis| placed.used.axes[axis] && (seen[axis] - before[axis]).abs() > 0.01);
  (state.scrollers, state.axes) = (placed.scrollers, placed.used.axes);
  let [valid, visible, _] = chosen.visibility;
  let veiled = |node: usize| {
    (cx.styles.values[node].get(Prop::VISIBILITY)).is_some_and(|value| value != "visible")
  };
  state.veiled = hides && overflows
    || valid && chosen.area.is_some() && state.target.is_none()
    || visible && state.target.is_some_and(veiled);
  (state.clips, state.hosts) = (Vec::new(), Vec::new());
  if let Some(target) = state.target.filter(|_| visible) {
    // The page clips its anchors at the edges of the viewport, whose size is in place of the borders.
    state
      .clips
      .extend(state.scrollers.iter().map(|&id| match id {
        0 => (0, [0., cx.viewport[0], cx.viewport[1], 0.]),
        id => (id, cx.styles.lengths[id].borders(&cx.styles.values[id], 0.)),
      }));
    let boxes = (cx.graph.nodes.as_ref()).map(|nodes| &nodes.boxes);
    let boxed = |at: &usize| boxes.is_some_and(|boxes| boxes.contains(at));
    state.hosts.extend(cx.up(target).filter(boxed));
  }
  state.at = seen;
  state.size = inner;
  state.targets = cx.reads.take();
  state.missed = cx.missed.take();
  state.vars = Some(cx.styles.vars[id].clone());
  let mut used = placed.used;
  if !state.scrolls() {
    state.hidden.set(state.veiled);
  }
  used.scrolls = state.scrolls() || state.veiled || hides;
  (used, moved || choose.is_some())
}
