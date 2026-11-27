pub(crate) mod anchor;
mod changes;
mod iframes;
mod lint;
mod motion;
mod page;
mod patch;
pub(crate) mod snap;
pub(crate) mod timeline;
mod visibility;

use super::element::{Build, element};
use super::layout::cull;
use super::paint::native;
use super::{ViewState, effects};
use crate::css::properties::{Prop, Style};
use crate::dom::{Document, LayoutMirror};
use crate::render::inline::InlineText;
use crate::style::ComputedStyles;
use motion::animate;
use renderer::{App, Window};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use visibility::{clips, in_view};

// What the stages of one page frame read: the page, the last frame and the box tree.
struct Cx<'a> {
  view: &'a Rc<ViewState>,
  doc: &'a Document,
  tree: &'a renderer::Tree,
  // Width, height, scale factor and font loads.
  frame: [f32; 4],
  // With culling, the part of the page near the viewport, where boxes can build in place.
  near: Option<[f32; 2]>,
  last: Option<&'a (u64, Option<usize>, Rc<ComputedStyles>)>,
  walked: &'a Rc<Vec<(u32, u32)>>,
  serial: u64,
  // The tree, the focus and the styles are the ones of the last frame.
  same: bool,
  visible: &'a [bool],
  // Animated boxes that draw in composited layers, and whether they turn.
  composited: &'a [(usize, bool)],
  // Pseudo-elements that animate only their opacity.
  pseudo_layers: &'a [usize],
  // The cascade values of animated pseudo-elements.
  restore_pseudos: &'a [(usize, Rc<[Style; 2]>)],
}

impl Cx<'_> {
  #[inline]
  fn shown(&self, id: usize) -> bool {
    self.visible.get(id) == Some(&true)
  }

  #[inline]
  fn place(
    &self,
    styles: &ComputedStyles,
    texts: &mut rustc_hash::FxHashMap<usize, Option<Rc<InlineText>>>,
    bounds: &LayoutMirror,
    id: usize,
  ) -> Result<(), usize> {
    self
      .view
      .place(self.doc, styles, texts, bounds, self.tree, self.near, id)
  }
}

impl ViewState {
  // `pointer_events` is the box of the window glue that takes the pointer for the page.
  pub(crate) fn render_page(
    self: &Rc<Self>,
    tree: &renderer::Tree,
    window: &mut Window,
    cx: &mut App,
    pointer_events: renderer::Node,
  ) -> renderer::Child {
    let started = std::time::Instant::now();
    {
      let mut store = self.store.borrow_mut();
      let state = store.state(self.id);
      state.attach_window(window);
      state.geometry.unshift();
      state.geometry.sync_scroll();
      state.geometry.mirror.touch();
      changes::repaint(state, window);
    }
    let mut store = self.store.borrow_mut();
    let size = window.viewport_size();
    let width = f32::from(size.width);
    let height = f32::from(size.height);
    let resized = store.viewports.insert(self.id, [width, height]) != Some([width, height]);
    // The desktop is borrowed while it opens a window, so resize events go through a deferred
    // `Desktop::drive`.
    if resized && !store.resized.contains(&self.id) {
      store.resized.push(self.id);
      let runtime = self.runtime.clone();
      cx.defer(move |cx| runtime.drive(cx));
    }
    let inspectable = !store.app.packaged;
    let state = &mut *store;
    let Some(page) = state.pages.get_mut(&self.id) else {
      return renderer::node().size_full().into();
    };
    page.dirty = false;
    let mut doc = std::mem::take(&mut page.doc);
    let dom = &mut page.state;
    // Clear node-ID caches on reload or navigation: the new DOM reuses IDs. Stamp its layout revision on
    // the first frame.
    if dom.layout.layout_revision == 0 {
      window.forget_states();
      self.kept.take();
      self.patches.take();
      self.outs.take();
      self.kept_frame.take();
      self.walked.take();
      self.adjoined.take();
      self.svgs.take();
      self.fields.take();
      self.decoding.take();
      self.flat.take();
      self.parked.take();
      self.caret.take();
      self.mismatched.take();
      self.frames.take();
      self.terminals.take();
      self.culled.take();
      self.animated.take();
      self.reach.take();
    }
    // The drawings of ended terminal sessions go now, with their images and timers.
    if !self.terminals.borrow().is_empty() {
      let sessions = dom.media.terminals.borrow();
      self
        .terminals
        .borrow_mut()
        .retain(|session, _| sessions.contains(*session));
    }
    {
      // Redraw once per mismatch until a pass has none, to stop alternating sizes from causing endless
      // frames.
      let [current, seen] = &mut *self.mismatched.borrow_mut();
      if current.is_empty() || seen.len() > 1024 {
        seen.clear();
      }
      seen.extend(current.drain());
    }
    for image in dom.media.retired.drain(..) {
      window.release_image(image);
    }
    for (node, image) in dom.media.recycled.drain(..) {
      window.release_image(image.clone());
      if let Ok(image) = Arc::try_unwrap(image)
        && let Some(player) = state.players.get_mut(&self.id)
      {
        player.recycle(node as u64, image);
      }
    }
    dom.media.retired_nodes.clear();
    dom.media.recycled_nodes.clear();
    if std::mem::take(&mut dom.paint_requested) {
      self.paint.set(true);
    }
    self.captured.update(dom, window);
    let cached = dom.styles.computed.clone();
    let mut styles = dom.restyle(&mut doc, [width, height]);
    if cached.is_none_or(|cached| !std::rc::Rc::ptr_eq(&cached, &styles))
      && dom.load_fonts(&doc, &state.app.assets).0
    {
      styles = dom.restyle(&mut doc, [width, height]);
    }
    visibility::wake(self, &doc, dom, &styles);
    let mut children: Vec<_> = styles
      .children
      .iter()
      .map(|(&node, children)| (node, doc.nodes.compose_children(node, children.clone())))
      .collect();
    let seen = (doc.nodes.latest(), dom.interaction.focus);
    // Remove cull.seen's extra Rc during the frame so animated styles can swap values without copying.
    let last = dom.layout.cull.seen.take();
    let (walked, serial) = (dom.layout.cull.walk.snapshot(), dom.layout.cull.walk.serial);
    // Animated values that the last frame swapped in are gone from `styles`, so their nodes changed.
    let same = dom.layout.cull.swapped.is_empty()
      && last.as_ref().is_some_and(|(latest, focus, computed)| {
        (*latest, *focus) == seen && Rc::ptr_eq(computed, &styles)
      });
    let (mut visible, mut previous, flips) =
      visibility::update(&doc, dom, &styles, last.as_ref(), same, &walked, serial);
    let mut events = Vec::new();
    // The passes that boxes with anchors settle in show one time, or a moving anchor would move in each.
    let now = (dom.layout.anchors.now.take())
      .filter(|_| window.redrawing())
      .unwrap_or_else(std::time::Instant::now);
    if styles.anchors.is_some() {
      dom.layout.anchors.now = Some(now);
    }
    if dom.styles.motion.scrolls {
      let timelines = timeline::Timelines::new(&doc, &styles, &dom.tree, &dom.geometry);
      dom
        .styles
        .motion
        .measure(&styles, &|id, scroll| timelines.progress(id, scroll));
    }
    let bounds = &dom.geometry.mirror;
    let side = |id| {
      bounds
        .get(&id)
        .map_or(f32::INFINITY, |rect| rect[2].min(rect[3]))
    };
    let crate::style::Frame {
      mut animated,
      next,
      custom,
      pseudos,
      backdrops,
      mut painted,
    } = dom.styles.motion.update(
      &doc,
      &styles,
      &visible,
      &|id| in_view(bounds, [width, height], id, clips(&doc, &styles, id)),
      &side,
      now,
      dom.layout.container_pass,
      &mut events,
    );
    let scrolls = dom.styles.motion.scrolls;
    let mut images = HashMap::new();
    if let Some(gpu) = state.graphics.get_mut(&self.id) {
      for &id in &dom.layout.cull.canvases {
        match gpu.image(id) {
          Ok(Some(image)) => {
            images.insert(id, image);
          }
          Err(error) => eprintln!("apps: canvas presentation failed: {error}"),
          _ => {}
        }
      }
    }
    let old = std::mem::replace(&mut *self.images.borrow_mut(), images);
    for (id, old) in old {
      if self
        .images
        .borrow()
        .get(&id)
        .is_none_or(|image| image.id != old.id)
      {
        window.release_image(old);
      }
    }
    if let Some(painted) = painted.take() {
      previous.get_or_insert(std::mem::replace(&mut visible, painted));
      dom.layout.cull.cascaded = false;
    }
    let flipped = previous.is_some();
    if let Some(previous) = previous {
      if walked.len() + dom.layout.cull.walk.order().len() < visible.len() {
        let walks = walked.iter().chain(dom.layout.cull.walk.order().iter());
        let nodes = walks
          .map(|&(id, _)| id as usize)
          .chain(previous.len()..visible.len());
        dom.geometry.mirror.hide(&visible, &previous, nodes);
      } else {
        dom
          .geometry
          .mirror
          .hide(&visible, &previous, 0..visible.len());
      }
    }
    *self.parked.borrow_mut() = dom
      .styles
      .motion
      .parked()
      .iter()
      .map(|&id| (id, clips(&doc, &styles, id).collect()))
      .collect();
    let mut lengths = motion::restyle(
      &mut doc,
      dom,
      &styles,
      &mut children,
      [width, height],
      custom,
      &mut animated,
    );
    let (composited, pseudo_layers) = motion::layers(self, &doc, &styles, &animated, &pseudos);
    let compositing = !animated.is_empty()
      && composited.len() == animated.len()
      && pseudo_layers.len() == pseudos.len()
      && backdrops.is_empty();
    animate(self, dom, next, now, compositing, window, cx);
    // Paint and the next frame read the cascade values, so they go back to `computed` after the frame.
    let taken = !pseudos.is_empty() || !animated.is_empty() || !lengths.is_empty();
    let (mut restore, restore_pseudos, paint) =
      motion::swap(dom, &mut styles, animated, pseudos, &mut lengths);
    let relaid = !restore.is_empty() || !lengths.is_empty();
    let frame = [width, height, window.scale_factor(), dom.fonts.loads as f32];
    let near = dom.layout.cull.enabled.then(|| {
      let moved = dom.layout.cull.scrolled.get(&0).copied().unwrap_or(0.);
      [moved - height, moved + 2. * height]
    });
    let fcx = Cx {
      view: self,
      doc: &doc,
      tree,
      frame,
      near,
      last: last.as_ref(),
      walked: &walked,
      serial,
      same,
      visible: &visible,
      composited: &composited,
      pseudo_layers: &pseudo_layers,
      restore_pseudos: &restore_pseudos,
    };
    visibility::forget(&fcx, dom, &styles, flips, flipped, window);
    native::sync(&self.controls, &doc, &styles.values);
    iframes::sync(
      &fcx,
      &dom.layout.cull.iframes,
      &styles,
      &paint,
      window,
      inspectable,
    );
    let (html, body, background) =
      page::canvas(&fcx, &styles, || state.backgrounds.get(&self.id).copied());
    // Never set a transparent window's native color: it prevents later transparency.
    #[cfg(target_os = "macos")]
    if background.1 == 255
      && state
        .backgrounds
        .get(&self.id)
        .is_none_or(|color| color.1 != 0)
      && self.canvas.replace(background) != background
    {
      window.change_window(renderer::WindowChange::Background(renderer::Color::hex(
        (background.0 << 8) | 255,
      )));
    }
    dom
      .layout
      .layout_nodes
      .resize_with(doc.nodes.len(), || None);
    let mut info = std::mem::take(&mut dom.layout.cull.info);
    let unchanged = same
      && !relaid
      && paint.iter().all(|(id, style)| {
        let flags = cull::flags(&doc.nodes[*id].tag, style, &styles.lengths[*id], height)
          & (cull::MOVES_CONTENT | cull::UNCULLABLE | cull::PINNED);
        info
          .get(*id)
          .is_some_and(|info| info.flags & flags == flags)
      });
    let settled = unchanged && !dom.layout.container_pass;
    const PER_FRAME: u32 = cull::CHANGED
      | cull::CHILDREN_CHANGED
      | cull::GEOMETRY_CHANGED
      | cull::COMPOSITED
      | cull::COMPOSITED_PSEUDO
      | cull::TRANSFORMED
      | cull::ANIMATED;
    let mut fresh = match dom.layout.cull.fresh.take() {
      Some(mut fresh) => {
        for id in fresh.drain(..) {
          if let Some(info) = info.get_mut(id) {
            info.flags &= !PER_FRAME;
          }
        }
        Some(fresh)
      }
      None => {
        for info in &mut info {
          info.flags &= !PER_FRAME;
        }
        Some(Vec::new())
      }
    };
    let (changed, sources, framed) = if unchanged {
      dom
        .layout
        .cull
        .painted
        .extend(paint.iter().map(|(id, _)| *id));
      dom.layout.cull.painted.sort_unstable();
      dom.layout.cull.painted.dedup();
      (Vec::new(), None, false)
    } else {
      changes::detect(&fcx, dom, &styles, &paint, &mut info, &mut fresh)
    };
    // Changes built in place move no box, so boxes that read the last frame can keep it, also in a reflow.
    let quiet = (unchanged
      || sources
        .is_some_and(|ids: Vec<usize>| ids.iter().all(|&id| dom.layout.cull.walk.contains(id))))
      && !relaid
      && !flipped
      && !framed
      && changed.is_empty()
      && !dom.layout.container_pass
      && paint
        .iter()
        .map(|(id, _)| *id)
        .chain(restore_pseudos.iter().map(|(id, _)| *id))
        .all(|id| {
          dom
            .layout
            .layout_nodes
            .get(id)
            .and_then(Option::as_ref)
            .is_some_and(|node| (node.flags | node.marks) & cull::READS_LAST_FRAME == 0)
        });
    self.composited_transforms.borrow_mut().clear();
    self
      .promote
      .set((!composited.is_empty()).then(|| (now + std::time::Duration::from_millis(300), false)));
    let relayouts = changes::animate(
      &fcx,
      dom,
      &mut styles,
      &mut info,
      &mut fresh,
      paint,
      &mut restore,
    );
    changes::remeasure(&fcx, dom, &styles, &changed, &mut info, &mut fresh);
    let warnings = if inspectable && !dom.styles.linted {
      lint::lint(&doc, dom, &styles, &changed)
    } else {
      Vec::new()
    };
    let viewport = page::viewport(&doc, dom, &styles, html, body, background);
    if !dom.layout.container_pass {
      let mut moved = 0.;
      dom.layout.cull.scrolled.clear();
      for (&id, handle) in &dom.geometry.scroll {
        let offset = handle.offset();
        let last = dom.layout.cull.offsets.insert(id, offset).unwrap_or(offset);
        moved += f32::from((offset.x - last.x).abs() + (offset.y - last.y).abs());
        if offset.y != last.y {
          dom
            .layout
            .cull
            .scrolled
            .insert(id, f32::from(last.y - offset.y));
        }
      }
      dom.layout.cull.resized = dom.layout.cull.frame[..2] != frame[..2];
      dom.layout.cull.enabled =
        !std::mem::take(&mut dom.layout.cull.off) && dom.layout.cull.frame[2..] == frame[2..];
      if moved >= height / 2. {
        self.kept.borrow_mut().retain(|_, kept| !kept.culled);
      }
      dom.layout.cull.frame = frame;
    }
    let zone = dom.layout.cull.enabled.then(|| {
      let moved = dom.layout.cull.scrolled.get(&0).copied().unwrap_or(0.);
      Rc::new(cull::Zone {
        top: moved,
        bottom: height + moved,
        clips: Vec::new(),
        resized: dom.layout.cull.resized,
      })
    });
    let mut left = Vec::new();
    let top_layer: Vec<usize> = dom
      .interaction
      .top_layer
      .iter()
      .copied()
      .filter(|&id| {
        let state = doc.nodes[id].state;
        let kept = state & crate::dom::TOP_LAYER != 0
          || visible.get(id) == Some(&true) && styles.values[id].get(Prop::OVERLAY) == Some("auto");
        if !kept && state & crate::dom::LEAVING != 0 {
          left.push(id);
        }
        (kept || doc.nodes[id].tag.starts_with("::"))
          && visible.get(id) == Some(&true)
          && doc.nodes[id].tag != "html"
      })
      .collect();
    dom.layout.measurements.pixel_ratio = window.scale_factor();
    let measured = &dom.layout.measurements;
    for (&id, &shown) in &measured.shown {
      if let Some(info) = info.get_mut(id) {
        info.flags = info.flags & !cull::SHOWN | (u32::from(shown) * cull::SHOWN);
      }
    }
    // A script reads geometry in a forced box, so neither it nor a box around it becomes a spacer.
    for &id in &measured.forced {
      let mut node = Some(id);
      while let Some(id) = node {
        if let Some(info) = info.get_mut(id) {
          info.flags |= cull::SHOWN;
        }
        node = dom.tree.flat_parent(&doc, &styles.children, id);
      }
    }
    let autos = !dom.layout.autos.is_empty();
    let anchored = styles.anchors.is_some() || !dom.layout.anchors.boxes.is_empty();
    let measured = RefCell::new(std::mem::take(&mut dom.layout.measurements));
    let walk = dom.styles.walk.snapshot();
    drop(store);
    if !warnings.is_empty() {
      self.runtime.lint(warnings);
    }
    let text_system = window.text_system().clone();
    let ids = {
      let mut ids = self.ids.borrow_mut();
      if ids.0 != doc.nodes.latest() {
        *ids = (doc.nodes.latest(), Rc::new(effects::Ids::of(walk)));
      }
      ids.1.clone()
    };
    let reads = RefCell::new(std::mem::take(&mut *self.reads.borrow_mut()));
    let mut build = Build {
      doc: &doc,
      styles: &styles,
      text: &text_system,
      pixel_ratio: window.scale_factor(),
      view: self,
      ancestors: Rc::default(),
      flex_parent: false,
      measured: &measured,
      perspective: None,
      siblings: &RefCell::default(),
      breaks: &RefCell::default(),
      walks: &RefCell::default(),
      adjoins: &RefCell::default(),
      blocks: &RefCell::default(),
      ids: &ids,
      info: &info,
      zone,
      columned: false,
      space: false,
      top_layer: &top_layer,
      tree,
      culls: &Cell::new(0),
      port: [width - viewport.space[1], height - viewport.space[0]],
      resized: false,
      settled: false,
      reads: &reads,
      epoch: 0,
      #[cfg(debug_assertions)]
      prints: &RefCell::default(),
      #[cfg(debug_assertions)]
      checking: false,
      #[cfg(debug_assertions)]
      extra: Default::default(),
      #[cfg(debug_assertions)]
      plan: Default::default(),
    };
    {
      let frame = [build.port[0], build.port[1], frame[2], frame[3]];
      let mut kept = self.kept_frame.borrow_mut();
      let turned = self.rtl.replace(viewport.rtl) != viewport.rtl;
      if kept.0[2..] != frame[2..] || kept.1 != top_layer || turned {
        kept.1 = top_layer.clone();
        self.kept.borrow_mut().clear();
        self.plans.borrow_mut().clear();
        self.shared.borrow_mut().clear();
      }
      {
        let mut records = self.kept.borrow_mut();
        if records.is_empty() {
          records.reserve(doc.nodes.len().min(1 << 20) / 2);
        }
      }
      let swapped = !restore.is_empty() || !restore_pseudos.is_empty() || !lengths.is_empty();
      let (epoch, last) = self.epoch.get();
      build.epoch = epoch + u64::from(!same || swapped || last || kept.0 != frame);
      self.epoch.set((build.epoch, swapped));
      build.resized = kept.0[..2] != frame[..2];
      build.settled =
        (settled && !self.reflowing.get() || quiet) && !build.resized && !window.redrawing();
      kept.0 = frame;
    }
    self.store.borrow_mut().state(self.id).layout.builds += 1;
    #[cfg(debug_assertions)]
    let zone = build.zone.clone();
    let port = build.port;
    let root = element(&build, 0, port, port, None);
    build.zone = None;
    let mut overlay = page::overlay(&build, &backdrops);
    overlay.extend(self.runtime.clone().popup().map(Into::into));
    let transition = (top_layer.iter()).any(|&id| doc.nodes[id].tag.starts_with("::"));
    if !same {
      patch::patch(&build, window);
    }
    patch::place(
      &build,
      window,
      #[cfg(debug_assertions)]
      zone,
      #[cfg(debug_assertions)]
      port,
    );
    drop(build);
    let mut reads = reads.into_inner();
    reads.clear();
    *self.reads.borrow_mut() = reads;
    for (node, children) in children.into_iter().rev() {
      doc.nodes.compose_children(node, children);
    }
    let repass = {
      let mut store = self.store.borrow_mut();
      let dom = store.state(self.id);
      for id in dom.styles.unanchored.drain(..) {
        window.suppress_anchoring(id);
      }
      #[cfg(debug_assertions)]
      if let Some(fresh) = &fresh {
        let fresh: rustc_hash::FxHashSet<usize> = fresh.iter().copied().collect();
        assert!(
          info
            .iter()
            .enumerate()
            .all(|(id, info)| info.flags & PER_FRAME == 0 || fresh.contains(&id)),
          "per-frame flags outside `fresh`"
        );
      }
      dom.layout.cull.info = info;
      dom.layout.cull.fresh = fresh;
      dom.layout.cull.visible = visible;
      dom.layout.measurements = measured.into_inner();
      let units = !lengths.is_empty();
      motion::restore(
        dom,
        &mut styles,
        restore,
        restore_pseudos,
        lengths,
        relayouts,
      );
      dom.layout.cull.seen = (!units).then(|| (seen.0, seen.1, styles.clone()));
      if taken {
        dom.styles.computed = Some(styles);
      }
      if !left.is_empty() {
        dom.interaction.top_layer.retain(|id| !left.contains(id));
        for id in left {
          if let Some(node) = doc.nodes.edit(id) {
            node.state &= !crate::dom::LEAVING;
          }
        }
        dom.styles.invalidate();
      }
      // Allow at most one extra pass per frame to bound container-query resize loops.
      let changed = dom.update_containers(&doc);
      let repass = changed && !dom.layout.container_pass;
      dom.layout.container_pass = repass;
      if let Some(page) = store.pages.get_mut(&self.id) {
        page.doc = doc;
      }
      repass
    };
    if repass {
      window.rebuild();
    }
    if !events.is_empty() {
      // Events of scroll-driven animations at the first frames of a page stalled its frames when they ran
      // inside the frame, so pages with scroll timelines run them after it.
      if self.reflowing.get() || scrolls {
        let runtime = self.runtime.clone();
        cx.defer(move |cx| runtime.animation_events(events, cx));
      } else {
        self.runtime.animation_events(events, cx);
        window.restart();
      }
    }
    if !window.redrawing() && !self.reflowing.get() && !repass {
      self.runtime.clone().queue_layout(window);
    }
    page::node(
      self,
      viewport,
      pointer_events,
      root,
      overlay,
      [autos, anchored, transition],
      started,
    )
  }
}
