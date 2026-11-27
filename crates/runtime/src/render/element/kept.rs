use crate::css::properties::Prop;
use std::rc::Rc;

// Log external reads through Build::log, or disable reuse of this box.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(debug_assertions, derive(Debug))]
pub(crate) enum Query {
    Margins,
    Parent(Prop),
    ParentFont,
    FlatParent,
    FlatParentPadding,
    StickyParent,
    Break,
    UserSelect,
    Positioner(bool),
    Backdrop(u32),
    // The state of the scroller walk when it reaches the box that holds the read.
    Scroller(u8),
    Editable,
    Cell,
    Shrunk,
    Uses(u32),
    Block(u32),
    Scroll(u32),
    Width(u32),
    Track(u32, u8),
    Baselines(u32),
    Atom(u32),
    Image(u32),
    Look(u32),
    Boxless(u32),
    RowFill,
    Collapse,
    Value(u32),
    Viewport,
    Theme,
    LineBase(u32),
    // Whether the parent of the node has a definite height.
    Definite(u32),
    Shrinks(u32),
    Spliced(u32),
    // The margins before the first line of the node, where its emphasis marks can reach.
    Room(u32),
}

impl Query {
    pub(super) fn moved(self, from: u32, to: u32) -> Option<Self> {
        let at = |node: u32| (node == from).then_some(to);
        Some(match self {
            Query::Uses(node) => Query::Uses(at(node)?),
            Query::Block(node) => Query::Block(at(node)?),
            Query::Scroll(node) => Query::Scroll(at(node)?),
            Query::Width(node) => Query::Width(at(node)?),
            Query::Backdrop(node) => Query::Backdrop(at(node)?),
            Query::Track(node, axis) => Query::Track(at(node)?, axis),
            Query::Baselines(node) => Query::Baselines(at(node)?),
            Query::Atom(node) => Query::Atom(at(node)?),
            Query::Image(node) => Query::Image(at(node)?),
            Query::Look(node) => Query::Look(at(node)?),
            Query::Boxless(node) => Query::Boxless(at(node)?),
            Query::Value(node) => Query::Value(at(node)?),
            Query::LineBase(node) => Query::LineBase(at(node)?),
            Query::Definite(node) => Query::Definite(at(node)?),
            Query::Shrinks(node) => Query::Shrinks(at(node)?),
            Query::Spliced(node) => Query::Spliced(at(node)?),
            Query::Room(node) => Query::Room(at(node)?),
            query => query,
        })
    }

    pub(super) fn host(self) -> bool {
        matches!(
            self,
            Query::Scroller(_)
                | Query::Block(_)
                | Query::Scroll(_)
                | Query::Width(_)
                | Query::Track(..)
                | Query::Baselines(_)
                | Query::Atom(_)
                | Query::Image(_)
                | Query::Viewport
                | Query::Theme
                | Query::Cell
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Read {
    pub(super) query: Query,
    pub(super) value: u64,
    pub(super) scope: u16,
    pub(super) reader: u16,
}

pub(crate) struct Patch {
    pub(crate) ancestors: Rc<[usize]>,
    pub(crate) containing: [f32; 2],
    pub(crate) container: [f32; 2],
    pub(crate) slot: Option<([f32; 4], super::super::layout::grid::Grid)>,
    pub(crate) flex_parent: bool,
    pub(crate) columned: bool,
    pub(crate) space: bool,
    pub(crate) perspective: Option<(f32, String)>,
    pub(crate) value: u64,
}

pub(crate) struct Shared {
    pub(super) plan: Rc<super::plan::Plan>,
    pub(super) reads: Box<[Read]>,
    pub(super) style: Rc<renderer::Style>,
}

pub(crate) struct Kept {
    pub(crate) call: Call,
    pub(crate) reads: Box<[Read]>,
    pub(super) ancestors: Rc<[usize]>,
    pub(super) epoch: std::cell::Cell<u64>,
    // The call drops a height that the style doesn't read, but a build in place needs it: a stretched image
    // takes its width from it.
    pub(super) containing: std::cell::Cell<[f32; 2]>,
    pub(crate) culled: bool,
    // The cell of a grid item, so it can build in place.
    pub(super) slot: Option<([f32; 4], super::super::layout::grid::Grid)>,
    #[cfg(debug_assertions)]
    pub(crate) print: u64,
    // The print that the records around hold. It stays when the box builds in place.
    #[cfg(debug_assertions)]
    pub(crate) seen: std::cell::Cell<u64>,
    #[cfg(debug_assertions)]
    pub(super) plan: Rc<[usize]>,
}

impl Kept {
    pub(crate) fn in_place(&self) -> Option<&Rc<[usize]>> {
        (!self.culled && (self.call.cell.is_none() || self.slot.is_some()))
            .then_some(&self.ancestors)
    }

    // The revision of the boxes that the record was built from.
    pub(crate) fn subtree(&self) -> u64 {
        self.call.subtree
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Call {
    pub(super) subtree: u64,
    pub(super) containing: [f32; 2],
    pub(super) container: [f32; 2],
    pub(super) flex_parent: bool,
    pub(super) columned: bool,
    pub(super) space: bool,
    pub(super) cell: Option<(bool, bool, u64)>,
}

#[derive(PartialEq)]
pub(super) enum Replay {
    Same,
    Close,
    Changed,
}

pub(crate) fn hashed(value: impl std::hash::Hash) -> u64 {
    use std::hash::BuildHasher;
    rustc_hash::FxBuildHasher.hash_one(value)
}
