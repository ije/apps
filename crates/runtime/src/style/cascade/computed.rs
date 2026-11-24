use crate::css::{Lengths, Prop, Registry, Style, Vars};
use crate::style::cascade::Container;
use crate::style::cascade::shadow::Shadowed;
use crate::style::chunked::Chunked;
use crate::style::keyframes::Animation;
use rustc_hash::FxHashMap;
use std::rc::Rc;
pub(crate) const DISPLAY_NONE: u16 = 1;
pub(crate) const DISPLAY_CONTENTS: u16 = 2;
pub(crate) const INVISIBLE: u16 = 4;
pub(crate) const SELECT_NONE: u16 = 8;
pub(crate) const SELECT_ALL: u16 = 16;
pub(crate) const SELECT_TEXT: u16 = 32;
pub(crate) const COUNTED: u16 = 64;
pub(crate) const AGENT_DISPLAY: u16 = 128;
// Table parts lay out as grids (tables), `contents` (row groups, rows) and blocks (cells, captions). Their role,
// the index of their display here, is in the high byte of the style flags. Rows and row groups outside a table
// make a table of their own, like the anonymous table around them in browsers. The last two roles are blocks whose
// children are all table parts, so they lay out as the anonymous table around those parts.
pub(crate) const TABLE_DISPLAYS: [&str; 15] = [
    "",
    "table",
    "inline-table",
    "block",
    "table-row",
    "table-row-group",
    "table-header-group",
    "table-footer-group",
    "table-cell",
    "table-caption",
    "table-row",
    "table-row-group",
    "table-header-group",
    "block",
    "inline-block",
];
pub(crate) const ROW: u16 = 4;
pub(crate) const HEADER: u16 = 6;
pub(crate) const FOOTER: u16 = 7;
pub(crate) const CELL: u16 = 8;
pub(crate) const CAPTION: u16 = 9;

pub(crate) fn role(styles: &ComputedStyles, id: usize) -> u16 {
    styles.flags.get(id).map_or(0, |flags| flags >> 8)
}

pub(crate) fn table_box(role: u16) -> bool {
    matches!(role, 1..=3 | 10..)
}

/// A table cell or caption that a grid doesn't blockify: it takes percentages from its table, not from a grid area.
pub(crate) fn table_part(doc: &crate::dom::Document, styles: &ComputedStyles, id: usize) -> bool {
    matches!(role(styles, id), CELL | CAPTION)
        && doc.nodes[id].parent.is_none_or(|parent| {
            styles.values[parent]
                .get(Prop::DISPLAY)
                .is_none_or(|display| !display.ends_with("grid"))
        })
}

impl ComputedStyles {
    // Gives a new detached node the styles of an unstyled node, as a full pass does, and grows every list.
    pub(crate) fn place(&mut self, id: usize, len: usize) {
        let (style, vars) = empty();
        self.values.resize(len, style.clone());
        self.values[id] = style;
        self.vars.resize(len, vars.clone());
        self.vars[id] = vars;
        self.flags.resize(len, 0);
        self.flags[id] = 0;
        self.lengths.grow(len);
        self.lengths.unset(id);
        self.user_select.resize(len, (UserSelect::None, 0));
        self.user_select[id] = (UserSelect::None, 0);
    }

    // The children of a node in the flat tree: a shadow host shows its shadow tree, and a slot its assigned nodes.
    pub(crate) fn flat_children<'a>(
        &'a self,
        doc: &'a crate::dom::Document,
        id: usize,
    ) -> &'a [usize] {
        self.children.get(&id).unwrap_or(&doc.nodes[id].children)
    }

    pub(crate) fn blended(&self) -> bool {
        self.blended.get().unwrap_or_else(|| {
            let blended = self.values.iter().any(|style| {
                style
                    .get(Prop::MIX_BLEND_MODE)
                    .is_some_and(|value| value != "normal")
            });
            self.blended.set(Some(blended));
            blended
        })
    }
}

thread_local! {
    static EMPTY: (Rc<Style>, Vars) = Default::default();
}

pub(crate) fn empty() -> (Rc<Style>, Vars) {
    EMPTY.with(Clone::clone)
}

#[derive(Clone, Default)]
pub(crate) struct NodeLengths {
    pub(crate) ids: Chunked<u32>,
    pub(crate) list: Vec<Lengths>,
    pub(crate) last: u32,
}

impl NodeLengths {
    pub(crate) fn reset(&mut self, count: usize, lengths: Lengths) {
        self.ids.clear();
        self.ids.resize(count, 0);
        self.list.clear();
        self.list.push(lengths);
        self.last = 0;
    }

    pub(crate) fn set(&mut self, id: usize, lengths: Lengths) {
        if self.list[self.last as usize] != lengths {
            // Only the newest values are searched, or a page with a font size per node gets quadratic.
            let mut newest = self.list.iter().enumerate().rev().take(32);
            self.last = match newest.find(|(_, old)| **old == lengths) {
                Some((index, _)) => index as u32,
                None => {
                    self.list.push(lengths);
                    self.list.len() as u32 - 1
                }
            };
        }
        self.ids.set(id, self.last, u32::eq);
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Lengths> {
        self.ids.get(id).map(|&index| &self.list[index as usize])
    }

    pub(crate) fn grow(&mut self, count: usize) {
        self.ids.resize(count, 0);
    }

    pub(crate) fn unset(&mut self, id: usize) {
        self.ids[id] = 0;
    }

    pub(crate) fn list_len(&self) -> usize {
        self.list.len()
    }
}

impl std::ops::Index<usize> for NodeLengths {
    type Output = Lengths;

    fn index(&self, id: usize) -> &Lengths {
        &self.list[self.ids[id] as usize]
    }
}

#[derive(Clone, Default)]
pub(crate) struct ComputedStyles {
    pub(crate) blended: std::cell::Cell<Option<bool>>,
    pub values: Chunked<Rc<Style>>,
    pub vars: Chunked<Vars>,
    pub flags: Chunked<u16>,
    pub lengths: NodeLengths,
    pub pseudos: FxHashMap<usize, Rc<[Style; 2]>>,
    pub text_pseudos: FxHashMap<usize, Rc<[Option<Style>; 4]>>,
    pub backdrops: FxHashMap<usize, Rc<Style>>,
    pub backdrop_starts: FxHashMap<usize, Rc<Style>>,
    pub scrollbars: FxHashMap<usize, Rc<[Option<Style>; 5]>>,
    pub selections: FxHashMap<usize, Rc<Style>>,
    pub user_select: Chunked<(UserSelect, usize)>,
    pub animations: FxHashMap<usize, Rc<[Animation]>>,
    // Properties whose winning declaration is important, so animations don't change them.
    pub important: FxHashMap<usize, Rc<[Prop]>>,
    pub children: FxHashMap<usize, Vec<usize>>,
    pub slotted: FxHashMap<usize, Shadowed>,
    pub parts: FxHashMap<usize, Shadowed>,
    pub exported: FxHashMap<usize, Shadowed>,
    pub starting: FxHashMap<usize, Rc<Style>>,
    pub list_items: FxHashMap<usize, i64>,
    // The declared `content` of `::before` and `::after` where it has a counter. Their styles hold the text of
    // the last pass.
    pub(crate) counting: FxHashMap<usize, [Option<Rc<str>>; 2]>,
    pub registry: Rc<Registry>,
    pub authored: FxHashMap<usize, Rc<Style>>,
    // The own flags of the nodes that an anonymous table changed. Their own styles are in `authored`.
    pub(crate) anonymous: FxHashMap<usize, u16>,
    pub(crate) containers: Chunked<Option<Rc<Container>>>,
    pub(crate) counted: bool,
    // The node revision of the document that this pass styled.
    pub(crate) revision: u64,
    // The native theme bits that this pass styled with.
    pub(crate) theme: u32,
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}
