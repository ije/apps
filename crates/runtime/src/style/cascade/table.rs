use crate::css::Style;
use crate::style::cascade::memo::StyleRef;
use rustc_hash::FxHashMap;
use std::rc::Rc;
pub(crate) type CellKey = (StyleRef, [(u32, Option<[Option<Rc<str>>; 2]>); 4]);

#[derive(PartialEq)]
pub(crate) struct Line {
    pub(crate) row: (usize, StyleRef),
    pub(crate) group: Option<(usize, StyleRef)>,
    pub(crate) cells: Vec<(usize, StyleRef, usize)>,
}

#[derive(Default)]
pub(crate) struct CollapsedTable {
    pub(crate) pass: u64,
    pub(crate) size: [f32; 2],
    pub(crate) table: Option<(StyleRef, [u32; 4], Rc<Style>)>,
    pub(crate) lines: Vec<Line>,
    pub(crate) adjusted: Vec<(Vec<Rc<Style>>, [f32; 4])>,
    pub(crate) styles: FxHashMap<CellKey, Rc<Style>>,
}
