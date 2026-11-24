pub(crate) mod changes;
pub(crate) mod lint;
pub(crate) mod motion;
pub(crate) mod patch;
pub(crate) mod pointer;
pub(crate) mod visibility;

use super::ViewState;
use crate::dom::{Document, LayoutMirror};
use crate::render::inline::InlineText;
use crate::style::ComputedStyles;
use std::rc::Rc;

// What the stages of one page frame read: the page, the last frame and the box tree.
pub(crate) struct Cx<'a> {
    pub(crate) view: &'a Rc<ViewState>,
    pub(crate) doc: &'a Document,
    pub(crate) tree: &'a renderer::Tree,
    // Width, height, scale factor and font loads.
    pub(crate) frame: [f32; 4],
    // With culling, the part of the page near the viewport, where boxes can build in place.
    pub(crate) near: Option<[f32; 2]>,
    pub(crate) last: Option<&'a (u64, Option<usize>, Rc<ComputedStyles>)>,
    pub(crate) walked: &'a Rc<Vec<(u32, u32)>>,
    pub(crate) serial: u64,
}

impl Cx<'_> {
    #[inline]
    pub(crate) fn place(
        &self,
        styles: &ComputedStyles,
        texts: &mut rustc_hash::FxHashMap<usize, Option<Rc<InlineText>>>,
        bounds: &LayoutMirror,
        id: usize,
    ) -> Result<(), usize> {
        (self.view).place(self.doc, styles, texts, bounds, self.tree, self.near, id)
    }
}
