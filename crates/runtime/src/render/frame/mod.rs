pub(crate) mod changes;
pub(crate) mod iframes;
pub(crate) mod lint;
pub(crate) mod motion;
pub(crate) mod page;
pub(crate) mod patch;
pub(crate) mod pointer;
pub(crate) mod visibility;

use super::ViewState;
use crate::css::properties::Style;
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
    // The tree, the focus and the styles are the ones of the last frame.
    pub(crate) same: bool,
    pub(crate) visible: &'a [bool],
    // Animated boxes that draw in composited layers, and whether they turn.
    pub(crate) composited: &'a [(usize, bool)],
    // Pseudo-elements that animate only their opacity.
    pub(crate) pseudo_layers: &'a [usize],
    // The cascade values of animated pseudo-elements.
    pub(crate) restore_pseudos: &'a [(usize, Rc<[Style; 2]>)],
}

impl Cx<'_> {
    #[inline]
    pub(crate) fn shown(&self, id: usize) -> bool {
        self.visible.get(id) == Some(&true)
    }

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
