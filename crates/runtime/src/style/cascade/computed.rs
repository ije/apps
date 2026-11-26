use crate::css::{
    properties::Prop,
    properties::Registry,
    properties::Style,
    properties::Vars,
    value::{tokens::number, units::Lengths},
};
use crate::style::cascade::Container;
use crate::style::cascade::shadow::Shadowed;
use crate::style::chunked::Chunked;
use crate::style::keyframes::Animation;
use rustc_hash::{FxHashMap, FxHashSet};
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
    // Gives a node that left the walk the styles of an unstyled node, and drops what was kept for it.
    pub(crate) fn leave(&mut self, id: usize, unstyled: &Rc<Style>, empty: &Vars) {
        self.values[id] = unstyled.clone();
        self.vars[id] = empty.clone();
        self.flags[id] = 0;
        self.lengths.unset(id);
        self.containers[id] = None;
        self.extras.remove(id);
    }

    // A fast pass starts from the styles of the last pass.
    pub(crate) fn start_from(&mut self, previous: &ComputedStyles, count: usize) {
        self.values.clone_from(&previous.values);
        self.vars.clone_from(&previous.vars);
        self.flags.clone_from(&previous.flags);
        self.lengths.clone_from(&previous.lengths);
        self.lengths.grow(count);
        self.extras.clone_from(&previous.extras);
        self.containers.clone_from(&previous.containers);
    }

    pub(crate) fn extras(&self, id: usize) -> &Extras {
        self.extras.get(id).unwrap_or(&NO_EXTRAS)
    }

    // What the anchors give a box.
    pub(crate) fn used(&self, id: usize) -> Option<&Rc<Used>> {
        let nodes = self.anchors.as_ref()?;
        self.anchored.get(&id).filter(|_| nodes.boxes.contains(&id))
    }

    // The style of a node without the values of its anchors, which start no transitions.
    pub(crate) fn unanchored(&self, id: usize) -> &Rc<Style> {
        let boxed = self
            .anchors
            .as_ref()
            .is_some_and(|nodes| nodes.boxes.contains(&id));
        (self.authored.get(&id).filter(|_| boxed)).unwrap_or(&self.values[id])
    }

    pub(crate) fn animations(&self) -> impl Iterator<Item = (usize, &Rc<[Animation]>)> {
        let ExtrasMap { map, animated, .. } = &self.extras;
        animated
            .iter()
            .map(|&id| (id, map[&id].animations.as_ref().unwrap()))
    }

    pub(crate) fn backdrops(&self) -> impl Iterator<Item = (usize, &Rc<Style>)> {
        let ExtrasMap { map, layered, .. } = &self.extras;
        layered
            .iter()
            .map(|&id| (id, map[&id].backdrop.as_ref().unwrap()))
    }

    // The styles of `::before` and `::after`, with the values of an animation frame while it draws.
    pub(crate) fn generated(&self, id: usize) -> Option<&Rc<[Style; 2]>> {
        if !self.frame_pseudos.is_empty()
            && let Some(pair) = self.frame_pseudos.get(&id)
        {
            return Some(pair);
        }
        self.extras(id).generated.as_ref()
    }

    // Puts the values of an animation frame in place of the cascade ones, and the cascade ones in the lists. A
    // second call with the same lists puts them back. Pseudo-elements without cascade styles are dropped.
    pub(crate) fn swap_frame(
        &mut self,
        values: &mut [(usize, Rc<Style>)],
        generated: &mut Vec<(usize, Rc<[Style; 2]>)>,
        lengths: &mut [(usize, Lengths)],
    ) {
        for (id, style) in values {
            std::mem::swap(&mut self.values[*id], style);
        }
        let frame = &mut self.frame_pseudos;
        generated.retain_mut(|(id, pair)| match frame.remove(id) {
            Some(animated) => {
                *pair = animated;
                true
            }
            None => match self.extras.get(*id).and_then(|kept| kept.generated.clone()) {
                Some(base) => {
                    frame.insert(*id, std::mem::replace(pair, base));
                    true
                }
                None => false,
            },
        });
        for (id, new) in lengths {
            let old = *self.lengths.get(*id).unwrap_or(new);
            self.lengths.set(*id, std::mem::replace(new, old));
        }
    }

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
    // The background colors that the cells of a table take from its `<col>` and `<colgroup>`, filled when asked.
    pub(crate) columns: std::cell::RefCell<FxHashMap<usize, Rc<FxHashMap<usize, usize>>>>,
    pub values: Chunked<Rc<Style>>,
    pub vars: Chunked<Vars>,
    pub flags: Chunked<u16>,
    pub lengths: NodeLengths,
    pub(crate) extras: ExtrasMap,
    // The `::before` and `::after` pairs of an animation frame, from `swap_frame()` until it is called again.
    frame_pseudos: FxHashMap<usize, Rc<[Style; 2]>>,
    pub user_select: Chunked<(UserSelect, usize)>,
    pub children: FxHashMap<usize, Vec<usize>>,
    pub list_items: FxHashMap<usize, i64>,
    pub registry: Rc<Registry>,
    pub authored: FxHashMap<usize, Rc<Style>>,
    // The own flags of the nodes that an anonymous table changed. Their own styles are in `authored`.
    pub(crate) anonymous: FxHashMap<usize, u16>,
    pub(crate) containers: Chunked<Option<Rc<Container>>>,
    pub(crate) counted: bool,
    pub(crate) anchors: Option<Rc<AnchorNodes>>,
    // What the anchors give the boxes.
    pub(crate) anchored: Rc<FxHashMap<usize, Rc<Used>>>,
    // The node revision of the document that this pass styled.
    pub(crate) revision: u64,
    // The native theme bits that this pass styled with.
    pub(crate) theme: u32,
}

// The nodes that anchor positioning places, with their own styles in `authored`, and the ones with an anchor name.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct AnchorNodes {
    pub(crate) boxes: FxHashSet<usize>,
    pub(crate) names: FxHashSet<usize>,
}

// What the anchors of a node give its style. `None` values are the initial ones.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Used {
    // The style of a position option.
    pub(crate) own: Option<Rc<Style>>,
    pub(crate) lengths: Vec<(Prop, Option<f32>)>,
    // The insets in the containing block, and those that scripts read, which are in the position area.
    pub(crate) insets: [Option<Option<f32>>; 4],
    pub(crate) shown: [Option<Option<f32>>; 4],
    // On the axes where the box aligns: whether its auto size fits its content, whether it stretches, and its place.
    pub(crate) fit: [bool; 2],
    pub(crate) stretch: [Option<bool>; 2],
    pub(crate) place: [Option<renderer::Aligned>; 2],
    // The axes on which the box follows scrolls of its anchor.
    pub(crate) axes: [bool; 2],
    // Layout places it, and scrolls can move or hide it.
    pub(crate) anchored: bool,
    pub(crate) scrolls: bool,
}

impl Used {
    // Puts the values into a style, for layout or for scripts.
    pub(crate) fn apply(&self, style: &mut Style, shown: bool) {
        if let Some(own) = &self.own {
            // An option changes only these properties.
            let placed = &crate::css::value::anchor::DESCRIPTORS;
            style.retain(|prop, _| !placed.contains(prop));
            let placed = own.own().filter(|(prop, _)| placed.contains(*prop));
            style.extend(placed.map(|(prop, value)| (prop, value.clone())));
        }
        let insets = if shown { &self.shown } else { &self.insets };
        let insets = (insets.iter().zip(crate::css::properties::INSETS))
            .filter_map(|(inset, prop)| Some((prop, (*inset)?)));
        for (prop, value) in insets.chain(self.lengths.iter().copied()) {
            match value {
                Some(value) => style.insert(prop, format!("{}px", number(f64::from(value)))),
                None => drop(style.remove(prop)),
            }
        }
        if shown {
            return;
        }
        for (axis, stretch) in self.stretch.iter().enumerate() {
            let [size, align] = [
                [Prop::WIDTH, Prop::JUSTIFY_SELF],
                [Prop::HEIGHT, Prop::ALIGN_SELF],
            ][axis];
            if let Some(stretch) = stretch {
                style.insert(align, if *stretch { "stretch" } else { "start" });
            }
            if self.fit[axis] {
                style.insert(size, "fit-content");
            }
        }
    }
}

// The rare results of a node, kept together so a pass adds, drops and copies them at once. Cached styles share
// them.
#[derive(Clone, Default)]
pub(crate) struct Extras {
    pub(crate) generated: Option<Rc<[Style; 2]>>,
    // `::marker`, `::first-letter`, `::first-line` and `::placeholder`.
    pub(crate) text: Option<Rc<[Option<Style>; 4]>>,
    pub(crate) backdrop: Option<Rc<Style>>,
    pub(crate) backdrop_start: Option<Rc<Style>>,
    pub(crate) scrollbars: Option<Rc<[Option<Style>; 5]>>,
    pub(crate) selection: Option<Rc<Style>>,
    pub(crate) animations: Option<Rc<[Animation]>>,
    // Properties whose winning declaration is important, so animations don't change them.
    pub(crate) important: Option<Rc<[Prop]>>,
    pub(crate) starting: Option<Rc<Style>>,
    pub(crate) slotted: Option<Shadowed>,
    pub(crate) parts: Option<Shadowed>,
    pub(crate) exported: Option<Shadowed>,
    // The declared `content` of `::before` and `::after` where it has a counter. Their styles hold the text of
    // the last pass.
    pub(crate) counting: [Option<Rc<str>>; 2],
}

// The extras of the nodes that have any, with the ids of the nodes that have animations and backdrops, which
// animation frames visit.
#[derive(Clone, Default)]
pub(crate) struct ExtrasMap {
    map: FxHashMap<usize, Rc<Extras>>,
    animated: FxHashSet<usize>,
    layered: FxHashSet<usize>,
}

impl ExtrasMap {
    pub(crate) fn get(&self, id: usize) -> Option<&Extras> {
        self.map.get(&id).map(|extras| &**extras)
    }

    pub(crate) fn put(&mut self, id: usize, extras: Rc<Extras>) {
        self.index(id, &extras);
        self.map.insert(id, extras);
    }

    pub(crate) fn remove(&mut self, id: usize) {
        if let Some(old) = self.map.remove(&id) {
            if old.animations.is_some() {
                self.animated.remove(&id);
            }
            if old.backdrop.is_some() {
                self.layered.remove(&id);
            }
        }
    }

    // Changes the extras of a node, and drops them when nothing is left.
    pub(crate) fn edit(&mut self, id: usize, edit: impl FnOnce(&mut Extras)) {
        let extras = self.map.entry(id).or_default();
        edit(Rc::make_mut(extras));
        let extras = extras.clone();
        match rare(&extras) {
            true => self.index(id, &extras),
            false => self.remove(id),
        }
    }

    fn index(&mut self, id: usize, extras: &Extras) {
        for (set, has) in [
            (&mut self.animated, extras.animations.is_some()),
            (&mut self.layered, extras.backdrop.is_some()),
        ] {
            match has {
                true => set.insert(id),
                false => set.remove(&id),
            };
        }
    }
}

// Whether the extras hold anything.
pub(crate) fn rare(extras: &Extras) -> bool {
    extras.generated.is_some()
        || extras.text.is_some()
        || extras.backdrop.is_some()
        || extras.backdrop_start.is_some()
        || extras.scrollbars.is_some()
        || extras.selection.is_some()
        || extras.animations.is_some()
        || extras.important.is_some()
        || extras.starting.is_some()
        || extras.slotted.is_some()
        || extras.parts.is_some()
        || extras.exported.is_some()
        || extras.counting.iter().any(Option::is_some)
}

pub(crate) const NO_EXTRAS: Extras = Extras {
    generated: None,
    text: None,
    backdrop: None,
    backdrop_start: None,
    scrollbars: None,
    selection: None,
    animations: None,
    important: None,
    starting: None,
    slotted: None,
    parts: None,
    exported: None,
    counting: [None, None],
};

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}
