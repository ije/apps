pub(crate) mod border;
pub(crate) mod children;
pub(crate) mod columns;
pub(crate) mod create;
pub(crate) mod cx;
pub(crate) mod kept;
pub(crate) mod margin;
#[cfg(debug_assertions)]
pub(crate) mod oracle;
pub(crate) mod paragraph;
pub(crate) mod plan;
pub(crate) mod pseudos;
pub(crate) mod query;
pub(crate) mod svg;

use self::create::create;
use self::kept::{Call, Kept, Patch, Query, Read, Replay, hashed};
use self::margin::{definite_line, float_side, margins, shrinks_around};
#[cfg(debug_assertions)]
use self::oracle::{TRACE, check, print};
use self::query::{
    LOOK, backdrop, baselines, box_parent, boxless, collapses, content_editable, insets,
    padding_start, parent_font, parent_style, passed, pictured, positioner, row_fill, scrolled,
    scroller, shrinks, starts_line, uses,
};
use super::layout::size::{broken_image, place, text_field};
use super::{ViewState, layout::cull};
use crate::css;
use crate::css::properties::Prop;
use crate::{dom::Document, style::ComputedStyles};
use renderer::Child;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub(crate) struct Build<'a> {
    pub(super) doc: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) text: &'a renderer::TextSystem,
    pub(super) pixel_ratio: f32,
    pub(super) view: &'a Rc<ViewState>,
    pub(super) ancestors: Rc<[usize]>,
    pub(super) flex_parent: bool,
    pub(super) measured: &'a RefCell<super::layout::measure::Measurements>,
    pub(super) perspective: Option<(f32, String)>,
    pub(super) siblings: &'a RefCell<HashMap<usize, Rc<Siblings>>>,
    // The index of each child of a parent with a `<br>`.
    pub(super) breaks: &'a RefCell<rustc_hash::FxHashMap<usize, Siblings>>,
    pub(super) walks: &'a RefCell<rustc_hash::FxHashMap<usize, Used>>,
    pub(super) adjoins: &'a RefCell<rustc_hash::FxHashMap<usize, Sets>>,
    pub(super) blocks: &'a RefCell<rustc_hash::FxHashMap<usize, [f32; 2]>>,
    pub(super) ids: &'a Rc<super::effects::Ids>,
    pub(super) info: &'a [cull::Info],
    pub(super) zone: Option<Rc<cull::Zone>>,
    // Inside a multi-column box, where boxes take break flags.
    pub(super) columned: bool,
    pub(super) top_layer: &'a [usize],
    pub(super) tree: &'a renderer::Tree,
    pub(super) culls: &'a std::cell::Cell<usize>,
    pub(super) resized: bool,
    pub(super) settled: bool,
    pub(super) reads: &'a RefCell<Vec<Read>>,
    pub(super) epoch: u64,
    #[cfg(debug_assertions)]
    pub(super) prints: &'a RefCell<Vec<u64>>,
    #[cfg(debug_assertions)]
    pub(super) checking: bool,
    #[cfg(debug_assertions)]
    pub(super) extra: std::cell::Cell<u64>,
    #[cfg(debug_assertions)]
    pub(super) plan: RefCell<Rc<[usize]>>,
}

impl<'a> Build<'a> {
    #[inline(never)]
    fn log(&self, query: Query, scope: usize, value: u64) {
        self.reads.borrow_mut().push(Read {
            query,
            value,
            scope: scope as u16,
            reader: self.ancestors.len() as u16,
        });
    }

    fn parent(&self, id: usize, prop: Prop) -> Option<&'a str> {
        let value = parent_style(self.doc, self.styles, id, prop);
        self.log(Query::Parent(prop), self.ancestors.len(), hashed(value));
        value
    }

    fn scope_of(&self, id: usize, node: usize) -> usize {
        std::iter::successors(Some(node), |&node| self.doc.nodes[node].parent)
            .find_map(|node| {
                if node == id {
                    Some(self.ancestors.len() + 1)
                } else {
                    self.ancestors
                        .iter()
                        .rposition(|&ancestor| ancestor == node)
                        .map(|index| index + 1)
                }
            })
            .unwrap_or(0)
    }

    fn block_size(&self, dom: Option<&crate::render::PageState>, block: usize) -> Option<[f32; 2]> {
        dom.and_then(|dom| dom.layout.blocks.get(&block))
            .map(|bounds| [f32::from(bounds.size.width), f32::from(bounds.size.height)])
            .or_else(|| self.blocks.borrow().get(&block).copied())
    }

    fn replay(
        &self,
        id: usize,
        [width, _]: [f32; 2],
        container: [f32; 2],
        cell: Option<[f32; 2]>,
        dom: Option<&crate::render::PageState>,
        read: &Read,
    ) -> Replay {
        let (doc, styles) = (self.doc, self.styles);
        let below = read.reader != 0;
        let now = match read.query {
            Query::Margins => {
                let mut units = styles.lengths[id];
                units.container = Some(container);
                hashed(
                    margins(self, id, &units, width, self.flex_parent, cell.is_some())
                        .map(f32::to_bits),
                )
            }
            Query::Parent(prop) => hashed(parent_style(doc, styles, id, prop)),
            Query::ParentFont => hashed(parent_font(doc, styles, id)),
            Query::FlatParent => {
                let parent = box_parent(doc, styles, id);
                hashed((
                    parent,
                    parent.and_then(|parent| styles.values[parent].get(Prop::DISPLAY)),
                ))
            }
            Query::FlatParentPadding => hashed(box_parent(doc, styles, id).map(|parent| {
                let (padding, rtl) = padding_start(styles, parent, width);
                (padding.to_bits(), rtl)
            })),
            Query::StickyParent => hashed(self.ancestors.last().map(|&parent| {
                (
                    parent,
                    insets(styles, parent, true, width).map(f32::to_bits),
                )
            })),
            Query::Break => hashed(starts_line(self, id)),
            Query::UserSelect => hashed(styles.user_select[id].0 as u8),
            Query::Positioner(fixed) => hashed(positioner(
                if fixed { "fixed" } else { "absolute" },
                self.ancestors.iter().rev().copied(),
                |ancestor| super::effects::establishes(&styles.values[ancestor]),
            )),
            Query::Backdrop(node) => hashed(backdrop(doc, styles, node as usize)),
            Query::Scroller(escape) => {
                let Some(handles) = dom.map(|dom| &dom.geometry.scroll) else {
                    return Replay::Changed;
                };
                let ancestors = self.ancestors.iter().rev().copied();
                let scroller = if below {
                    scroller(
                        doc,
                        styles,
                        std::iter::once(id).chain(ancestors),
                        handles,
                        escape,
                    )
                } else {
                    scroller(doc, styles, ancestors, handles, escape)
                };
                hashed((
                    scroller,
                    insets(styles, scroller, false, width).map(f32::to_bits),
                    handles.contains_key(&scroller),
                ))
            }
            Query::Editable => hashed(content_editable(doc, id).is_some()),
            Query::Cell => hashed(cell.map(|size| size.map(f32::to_bits))),
            Query::Shrunk => hashed(shrinks(doc, styles, id, !below).0),
            Query::Uses(svg) => hashed(uses(doc, styles, self.ids, svg as usize)),
            Query::Block(block) => hashed(
                self.block_size(dom, block as usize)
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Scroll(node) => hashed(scrolled(
                dom.and_then(|dom| dom.geometry.scroll.get(&(node as usize))),
            )),
            Query::Width(node) => hashed(
                dom.and_then(|dom| dom.layout.cull.widths.get(&(node as usize)))
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Track(node, axis) => {
                // The 0.5 px must match prepaint, which draws again only for a size more than half a pixel
                // off.
                let used = f32::from_bits(read.value as u32);
                return match dom
                    .and_then(|dom| dom.layout.cull.widths.get(&(node as usize)))
                    .map(|size| size[usize::from(axis)])
                {
                    // A node keeps its size, so a read that finds none was made without one too.
                    None => Replay::Same,
                    Some(size) if size.to_bits() == used.to_bits() => Replay::Same,
                    Some(size) if (size - used).abs() <= 0.5 => Replay::Close,
                    Some(_) => Replay::Changed,
                };
            }
            Query::Baselines(grid) => baselines(doc, dom, grid as usize),
            Query::Atom(node) => hashed(
                dom.and_then(|dom| dom.layout.atoms.get(&(node as usize)))
                    .map(|size| size.map(f32::to_bits)),
            ),
            Query::Image(node) => {
                let node = node as usize;
                let dom = dom.filter(|_| !broken_image(&doc.nodes[node]));
                pictured(
                    dom.and_then(|dom| dom.media.images.get(&node)),
                    dom.and_then(|dom| dom.media.svg_images.get(&node)),
                )
            }
            Query::Look(node) => hashed(doc.nodes[node as usize].state & LOOK),
            Query::Boxless(node) => hashed(boxless(doc, node as usize).map(|select| select.1)),
            Query::Spliced(node) => {
                hashed(crate::render::inline::spliced(doc, styles, node as usize))
            }
            Query::RowFill => {
                hashed(row_fill(doc, styles, id).map(|fill| fill.to_array().map(f32::to_bits)))
            }
            Query::Collapse => hashed(collapses(doc, styles, id)),
            Query::Value(node) => hashed(&doc.nodes[node as usize].value),
            Query::Viewport => hashed(styles.lengths[id].viewport.map(f32::to_bits)),
            Query::Theme => hashed(crate::native::theme_state() & 1),
            Query::LineBase(node) => hashed(definite_line(doc, styles, node as usize)),
            Query::Shrinks(node) => hashed(shrinks_around(doc, styles, node as usize)),
        };
        if now == read.value {
            Replay::Same
        } else {
            Replay::Changed
        }
    }
}

// The place of each list item among the list items of its parent.
pub(super) type Siblings = rustc_hash::FxHashMap<usize, usize>;

pub(super) type Used = Rc<rustc_hash::FxHashMap<usize, [f32; 2]>>;

pub(super) type Sets = ([f32; 2], [f32; 2], bool);

pub(crate) type Adjoined = ((u64, u32, Option<[u32; 2]>), Sets);

pub(crate) type Walked = ((u64, u32, [bool; 2], Option<[u32; 2]>), Used);

thread_local! {
    // Reserve a quarter of the stack for layout and paint, which recurse as deeply as build.
    pub(super) static STACK_LIMIT: usize = {
        let marker = 0u8;
        let here = &raw const marker as usize;
        #[cfg(target_os = "macos")]
        let (low, size) = unsafe {
            let size = libc::pthread_get_stacksize_np(libc::pthread_self());
            (libc::pthread_get_stackaddr_np(libc::pthread_self()) as usize - size, size)
        };
        #[cfg(target_os = "linux")]
        let (low, size) = unsafe {
            let mut attr = std::mem::zeroed();
            let (mut low, mut size) = (std::ptr::null_mut(), 0);
            if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
                libc::pthread_attr_getstack(&attr, &mut low, &mut size);
                libc::pthread_attr_destroy(&mut attr);
            }
            (low as usize, size)
        };
        // Assumes the 64 MB main stack that build.rs links on Windows.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let (low, size) = (here.saturating_sub(60 << 20), 64 << 20);
        (low + size / 4).min(here)
    };
}

pub(crate) fn element(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::layout::grid::Grid)>,
) -> Child {
    let info = build.info[id];
    let depth = build.ancestors.len();
    let mark = build.reads.borrow().len();
    #[cfg(debug_assertions)]
    let printed = build.prints.borrow().len();
    let text = build.doc.nodes[id].tag.starts_with('#');
    let storable = !text
        && build.perspective.is_none()
        && depth < 256
        && info.flags
            & (cull::ANIMATED
                | cull::HIDDEN
                | cull::MOVES_CONTENT
                | cull::COMPOSITED
                | cull::TRANSFORMED)
            == 0;
    let call = Call {
        subtree: info.subtree,
        containing: if info.flags & cull::READS_HEIGHT != 0 {
            containing
        } else {
            [containing[0], 0.]
        },
        container,
        flex_parent: build.flex_parent,
        columned: build.columned,
        cell: slot.as_ref().map(|(_, grid)| {
            use std::hash::{Hash, Hasher};
            let mut tracks = rustc_hash::FxHasher::default();
            for value in grid.columns.iter().chain(&grid.rows).chain(&grid.gap) {
                value.to_bits().hash(&mut tracks);
            }
            (grid.baseline, grid.first_item.is_some(), tracks.finish())
        }),
    };
    // Debug checks must not remove repaint entries, or the real build can skip the box.
    #[cfg(debug_assertions)]
    let repainted = if build.checking {
        build.view.repaint.borrow().contains(&id)
    } else {
        build.view.repaint.borrow_mut().remove(&id)
    };
    #[cfg(not(debug_assertions))]
    let repainted = build.view.repaint.borrow_mut().remove(&id);
    let keep = 'keep: {
        if !storable
            || repainted
            || build.resized && info.flags & cull::READS_VIEWPORT != 0
            || !build.settled && info.flags & cull::READS_LAST_FRAME != 0
            || !build.tree.contains(id)
        {
            break 'keep None;
        }
        let kept = build.view.kept.borrow();
        let Some(record) = kept.get(&id) else {
            break 'keep None;
        };
        let cell = slot.as_ref().map(|(bounds, _)| [bounds[2], bounds[3]]);
        // A float is never culled, so it stays kept far from the view.
        if !record.culled
            && float_side(&build.styles.values[id]).is_none()
            && let Some(zone) = &build.zone
        {
            let margin = build.styles.lengths[id].viewport[1];
            let reach = build.view.reach.borrow().get(id).copied();
            if !reach.is_some_and(|[bottom, top]| {
                bottom >= zone.top - margin && top <= zone.bottom + margin
            }) {
                break 'keep None;
            }
        }
        let mut close = false;
        // A debug check of a kept box keeps the positioned boxes in it that build in place after the build.
        #[cfg(debug_assertions)]
        let inside = build.checking
            && info.flags & cull::OUT_OF_FLOW != 0
            && build.view.outs.borrow().contains_key(&id)
            && Call {
                subtree: record.call.subtree,
                cell: call.cell,
                ..call
            } == record.call;
        #[cfg(not(debug_assertions))]
        let inside = false;
        let held = (record.call == call || inside) && {
            let fresh = record.epoch.get() == build.epoch;
            let host = build.view.host.borrow();
            let dom = host.page_state(build.view.id);
            record.reads.iter().all(|read| {
                fresh && !read.query.host()
                    || match build.replay(id, containing, container, cell, dom, read) {
                        Replay::Same => true,
                        Replay::Close => {
                            close = true;
                            true
                        }
                        Replay::Changed => false,
                    }
            })
        };
        #[cfg(debug_assertions)]
        assert!(
            held || !build.checking,
            "box {id} <{}> builds again inside a kept box: {}",
            build.doc.nodes[id].tag,
            if record.call == call {
                let host = build.view.host.borrow();
                let dom = host.page_state(build.view.id);
                format!(
                    "{:?} changed",
                    record
                        .reads
                        .iter()
                        .filter(
                            |read| build.replay(id, containing, container, cell, dom, read)
                                == Replay::Changed
                        )
                        .map(|read| read.query)
                        .collect::<Vec<_>>()
                )
            } else {
                "its call changed".into()
            }
        );
        if !held {
            break 'keep None;
        }
        record.epoch.set(build.epoch);
        record.containing.set(containing);
        if record.culled {
            build.culls.set(build.culls.get() + 1);
        }
        build
            .reads
            .borrow_mut()
            .extend(record.reads.iter().map(|read| Read {
                scope: if read.scope == 0 {
                    0
                } else {
                    (depth + 1).saturating_sub(usize::from(read.scope)).max(1) as u16
                },
                reader: depth as u16 + read.reader,
                query: passed(build.styles, id, read.query, read.reader != 0),
                ..*read
            }));
        #[cfg(debug_assertions)]
        build.prints.borrow_mut().push(match inside {
            true => record.seen.get(),
            false => {
                if !build.checking {
                    record.seen.set(record.print);
                }
                record.print
            }
        });
        // A record with culled boxes can come from a debug build in a zone, which a check without a zone can't
        // repeat. The zone checks of `plan` cover such boxes.
        Some(close || inside || record.culled)
    };
    if let Some(_close) = keep {
        #[cfg(debug_assertions)]
        if !_close && !build.view.around.borrow().contains(&id) {
            check(build, id, containing, container, slot, mark, printed);
        }
        return Child::Kept(id.into());
    }
    let culls = build.culls.get();
    #[cfg(debug_assertions)]
    let checking = build.checking;
    #[cfg(not(debug_assertions))]
    let checking = false;
    if !checking && !build.view.repaint.borrow().is_empty() {
        build.view.built.borrow_mut().insert(id);
    }
    let field = text_field(&build.doc.nodes[id], &build.styles.values[id]);
    let positioned = storable && info.flags & cull::OUT_OF_FLOW != 0;
    let patch = (field || positioned).then(|| Patch {
        ancestors: build.ancestors.clone(),
        containing,
        container,
        slot: slot.clone(),
        flex_parent: build.flex_parent,
        columned: build.columned,
        perspective: build.perspective.clone(),
        value: hashed(&build.doc.nodes[id].value),
    });
    let cell = slot.clone().filter(|_| storable);
    let child = create(build, id, containing, container, slot, None);
    #[cfg(debug_assertions)]
    let plan = build.plan.take();
    #[cfg(debug_assertions)]
    if !build.checking
        && *TRACE
        && let Some(name) = build.doc.nodes[id].attrs.get("id")
    {
        eprintln!("BUILT {name}");
    }
    let reads = {
        let below = |read: &Read| usize::from(read.reader) > depth;
        let mut log = build.reads.borrow_mut();
        let mut end = mark;
        for at in mark..log.len() {
            let read = log[at];
            if usize::from(read.scope) <= depth
                && !log[end.saturating_sub(16).max(mark)..end]
                    .iter()
                    .any(|old| old.query == read.query && below(old) == below(&read))
            {
                log[end] = read;
                end += 1;
            }
        }
        log.truncate(end);
        let reads = storable.then(|| {
            log[mark..]
                .iter()
                .map(|read| Read {
                    scope: if read.scope == 0 {
                        0
                    } else {
                        (depth + 1 - usize::from(read.scope)) as u16
                    },
                    reader: u16::from(below(read)),
                    ..*read
                })
                .collect()
        });
        for read in &mut log[mark..] {
            read.query = passed(build.styles, id, read.query, below(read));
        }
        reads
    };
    #[cfg(debug_assertions)]
    let print = print(build, printed, &child);
    if let Some(patch) = patch.filter(|_| !checking) {
        match field {
            true => build.view.patches.borrow_mut().insert(id, patch),
            false => build.view.outs.borrow_mut().insert(id, patch),
        };
    }
    if !text {
        let mut kept = build.view.kept.borrow_mut();
        match reads {
            Some(reads) => {
                kept.insert(
                    id,
                    Kept {
                        call,
                        reads,
                        ancestors: build.ancestors.clone(),
                        epoch: build.epoch.into(),
                        containing: containing.into(),
                        culled: build.culls.get() != culls,
                        slot: cell,
                        #[cfg(debug_assertions)]
                        print,
                        #[cfg(debug_assertions)]
                        seen: print.into(),
                        #[cfg(debug_assertions)]
                        plan,
                    },
                );
            }
            None => {
                kept.remove(&id);
            }
        }
    }
    child
}

pub(crate) fn repaint(build: &Build<'_>, id: usize) -> Option<renderer::Node> {
    let old = build.view.kept.borrow_mut().remove(&id)?;
    let (ancestors, containing, container, flex_parent) = (
        old.ancestors.clone(),
        old.containing.get(),
        old.call.container,
        old.call.flex_parent,
    );
    let mark = build.reads.borrow().len();
    let place = Build {
        ancestors: ancestors.clone(),
        flex_parent,
        columned: old.call.columned,
        perspective: None,
        zone: None,
        #[cfg(debug_assertions)]
        extra: Default::default(),
        #[cfg(debug_assertions)]
        plan: Default::default(),
        ..*build
    };
    #[cfg(debug_assertions)]
    let printed = place.prints.borrow().len();
    let bounds = old.slot.as_ref().map(|(bounds, _)| *bounds);
    let child = element(&place, id, containing, container, old.slot.clone());
    // A grid item takes its place in its cell, as its parent would give it.
    let child = match bounds {
        Some(bounds) => {
            let mut units = build.styles.lengths[id];
            units.container = Some(container);
            let margin = units.edges(
                &build.styles.values[id],
                css::properties::MARGIN,
                containing[0],
            );
            self::place(child, margin, build.pixel_ratio, bounds)
        }
        None => child,
    };
    #[cfg(debug_assertions)]
    place.prints.borrow_mut().truncate(printed);
    build.reads.borrow_mut().truncate(mark);
    let mut kept = build.view.kept.borrow_mut();
    if kept.get(&id).is_none_or(|record| record.reads != old.reads) {
        for ancestor in ancestors.iter() {
            kept.remove(ancestor);
        }
    }
    match child {
        Child::Node(node) => Some(*node),
        Child::Kept(_) | Child::Placed(..) => None,
    }
}
