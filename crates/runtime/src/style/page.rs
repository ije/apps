use super::{ComputedStyles, Motion, Shadows, StyleCache};
use crate::css::{Prop, Selector, Style, Stylesheet};
use crate::dom::Walk;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

// The style state of a page: the last pass, its caches, the animations and the selection modes.
#[derive(Default)]
pub(crate) struct Styles {
    pub(crate) computed: Option<Rc<ComputedStyles>>,
    pub(crate) previous: Option<Rc<ComputedStyles>>,
    pub(crate) walk: Walk,
    pub(crate) editable: Vec<usize>,
    pub(crate) inert: Vec<usize>,
    pub(crate) select_marks: Vec<u8>,
    // The pass that made the marks: its styles, modal subject and walk, and whether it flattened children.
    pub(crate) select_base: (usize, Option<usize>, u64, bool),
    pub(crate) shadow_walk: Rc<ShadowWalk>,
    pub(crate) epoch: u64,
    pub(crate) sheets: HashMap<Rc<str>, Rc<Stylesheet>>,
    pub(crate) inserted: Vec<(String, Rc<Stylesheet>, bool)>,
    pub(crate) cache: StyleCache,
    pub(crate) key: ([f32; 2], Option<f32>, bool, u32),
    pub(crate) queries: FxHashMap<Box<str>, Rc<[Selector]>>,
    pub(crate) linted: bool,
    pub(crate) warned: HashSet<String>,
    pub(crate) checked: FxHashSet<(u8, Prop, Rc<str>)>,
    // The values in `checked` by address. Each holds its value, so no other value takes the address.
    pub(crate) checked_values: FxHashMap<(usize, u8, Prop), Rc<str>>,
    pub(crate) motion: Motion,
    pub(crate) animated: HashMap<usize, Style>,
    pub(crate) animating: HashMap<usize, Rc<Style>>,
    pub(crate) animating_pseudos: HashMap<usize, Rc<[Style; 2]>>,
    pub(crate) recolored: Vec<usize>,
    // The nodes whose style moved or sized them since the last frame, which suppress scroll anchoring.
    pub(crate) unanchored: Vec<usize>,
}

#[derive(Default)]
pub(crate) struct ShadowWalk {
    pub(crate) tree: u64,
    pub(crate) count: usize,
    pub(crate) slots: FxHashMap<usize, Vec<usize>>,
    pub(crate) shadows: Rc<Shadows>,
}
