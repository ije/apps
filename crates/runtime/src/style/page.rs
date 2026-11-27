use super::{Motion, Shadows, StyleCache};
use crate::css::{properties::Prop, properties::Style, selector::Selector, sheet::Stylesheet};
use crate::dom::{Document, LayoutMirror, Walk};
use crate::style::cascade::computed::ComputedStyles;
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
    pub(crate) highlights: std::cell::RefCell<crate::style::cascade::Highlights>,
    pub(crate) key: Key,
    pub(crate) queries: FxHashMap<Box<str>, Rc<[Selector]>>,
    pub(crate) linted: bool,
    pub(crate) warned: HashSet<String>,
    pub(crate) checked: FxHashSet<(u8, Prop, Rc<str>)>,
    // The values in `checked` by address. Each holds its value, so no other value takes the address.
    pub(crate) checked_values: FxHashMap<(usize, u8, Prop), Rc<str>>,
    pub(crate) motion: Motion,
    // The timer that wakes the page for the next animation frame.
    pub(crate) wake: Option<(std::time::Instant, renderer::Timer)>,
    pub(crate) animated: HashMap<usize, Style>,
    pub(crate) animating: HashMap<usize, Rc<Style>>,
    pub(crate) animating_pseudos: HashMap<usize, Rc<[Style; 2]>>,
    // The units of the last frame where animated font sizes or zoom changed them.
    pub(crate) animating_units: Vec<(usize, crate::css::value::units::Lengths)>,
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
    // The slot nodes of the arena, while the page has shadow roots.
    pub(crate) known: Option<Vec<usize>>,
}

// The select marks of a node: hidden, editable, inert, and inert by its own attribute.
const HIDDEN: u8 = 1;
const EDITABLE: u8 = 2;
const INERT: u8 = 4;
const OWN_INERT: u8 = 8;

// What a style pass reads from the page.
pub(crate) struct Page<'a> {
    pub(crate) shadows: &'a FxHashMap<usize, usize>,
    pub(crate) hosts: &'a FxHashMap<usize, usize>,
    pub(crate) top_layer: &'a [usize],
    pub(crate) font_aliases: &'a HashMap<String, String>,
    pub(crate) text_system: Option<&'a renderer::TextSystem>,
    pub(crate) bounds: &'a mut LayoutMirror,
}

type Key = ([f32; 2], Option<f32>, bool, u32);

// A pass that `begin` found to be needed.
pub(crate) struct Pending {
    key: Key,
    animated: HashMap<usize, Style>,
}

impl Styles {
    // The styles of the last frame, also after a change that asks the next frame for new ones.
    pub(crate) fn last(&self) -> Option<&Rc<ComputedStyles>> {
        self.computed.as_ref().or(self.previous.as_ref())
    }

    pub(crate) fn invalidate(&mut self) {
        if let Some(computed) = self.computed.take() {
            self.previous = Some(computed);
        }
    }

    // The styles of the last pass, which can be old.
    pub(crate) fn latest(&self) -> Option<Rc<ComputedStyles>> {
        self.computed.clone().or_else(|| self.previous.clone())
    }

    // A node joins the arena with `len` nodes.
    pub(crate) fn place(&mut self, id: usize, len: usize) {
        if let Some(cached) = &mut self.computed {
            // Grow every per-node list here. Initialize detached nodes as a full style pass does.
            if let Some(computed) = Rc::get_mut(cached) {
                computed.place(id, len);
            } else if id >= cached.values.len() {
                self.invalidate();
            }
        }
    }

    // The last pass when it still holds, or else what the next pass needs.
    pub(crate) fn begin(
        &mut self,
        doc: &Document,
        viewport: [f32; 2],
        theme: u32,
        scale: Option<f32>,
        bounds: &mut LayoutMirror,
    ) -> Result<Rc<ComputedStyles>, Pending> {
        let key = (viewport, scale, theme & 32 != 0, theme);
        let animated = std::mem::take(&mut self.animated);
        // Resize observers read content boxes, which these values change without a new border box.
        if animated.values().any(|style| {
            !style.vars().is_empty()
                || style.own().any(|(prop, _)| {
                    let name = prop.name();
                    ["padding", "border", "overflow", "scrollbar"]
                        .iter()
                        .any(|start| name.starts_with(start))
                        || matches!(name, "display" | "font-size" | "zoom" | "box-sizing")
                })
        }) {
            bounds.restyled();
        }
        if animated.is_empty()
            && self.key == key
            && let Some(computed) = &self.computed
            && computed.values.len() == doc.nodes.len()
        {
            return Ok(computed.clone());
        }
        Err(Pending { key, animated })
    }

    pub(crate) fn update(
        &mut self,
        doc: &mut Document,
        Pending { key, animated }: Pending,
        page: Page,
    ) -> Rc<ComputedStyles> {
        let (viewport, scale, reduced_motion, theme) = key;
        let animating = !animated.is_empty();
        if self.shadow_walk.tree != doc.nodes.tree() || self.shadow_walk.count != page.shadows.len()
        {
            // A node gets a revision when it is placed, so the new slot nodes are among the changed ones.
            let known = match &self.shadow_walk.known {
                _ if page.shadows.is_empty() => None,
                Some(known) => {
                    let mut known = known.clone();
                    for id in doc.nodes.changed_since(self.shadow_walk.tree) {
                        if doc.nodes[id].tag == "slot" && !known.contains(&id) {
                            known.push(id);
                        }
                    }
                    known.retain(|&id| doc.nodes.get(id).is_some_and(|node| node.tag == "slot"));
                    Some(known)
                }
                None => Some(
                    (0..doc.nodes.len())
                        .filter(|&id| doc.nodes[id].tag == "slot")
                        .collect(),
                ),
            };
            let all = || {
                crate::dom::tree::slots(doc, page.shadows.iter().map(|(&host, &root)| (host, root)))
            };
            let slots = known
                .as_deref()
                .and_then(|known| {
                    crate::dom::tree::known_slots(doc, known, page.shadows, page.hosts)
                })
                .unwrap_or_else(all);
            debug_assert!(slots == all(), "slots of the known slot nodes differ");
            let assigned: rustc_hash::FxHashSet<usize> =
                slots.values().flatten().copied().collect();
            let mut composed = rustc_hash::FxHashMap::default();
            for (&host, &root) in page.shadows {
                composed.insert(
                    host,
                    std::iter::once(root)
                        .chain(
                            doc.nodes[host]
                                .children
                                .iter()
                                .copied()
                                .filter(|child| !assigned.contains(child)),
                        )
                        .collect(),
                );
            }
            for (&slot, nodes) in &slots {
                if !nodes.is_empty() {
                    composed.insert(
                        slot,
                        nodes
                            .iter()
                            .chain(&doc.nodes[slot].children)
                            .copied()
                            .collect(),
                    );
                }
            }
            let old = &self.shadow_walk.shadows.hosts;
            let hosts = if old.len() == page.hosts.len()
                && page
                    .hosts
                    .iter()
                    .all(|(root, host)| old.get(root) == Some(host))
            {
                old.clone()
            } else {
                Rc::new(
                    page.hosts
                        .iter()
                        .map(|(&root, &host)| (root, host))
                        .collect(),
                )
            };
            self.shadow_walk = Rc::new(super::ShadowWalk {
                tree: doc.nodes.tree(),
                count: page.shadows.len(),
                slots,
                known,
                shadows: Rc::new(super::Shadows {
                    roots: page.shadows.clone(),
                    hosts,
                    composed,
                }),
            });
        }
        let shadow_walk = self.shadow_walk.clone();
        let context = super::StyleContext {
            shadows: shadow_walk.shadows.clone(),
            reduced_motion,
            theme,
            scale: scale.unwrap_or(1.),
            aliases: page.font_aliases.clone(),
            animated,
            inserted: self
                .inserted
                .iter()
                .map(|(_, sheet, user)| (sheet.clone(), *user))
                .collect(),
            previous: self.computed.clone().or_else(|| self.previous.take()),
            top_layer: !page.top_layer.is_empty(),
        };
        let mut result = super::compute_styles(
            doc,
            viewport[0],
            viewport[1],
            page.text_system,
            &mut self.sheets,
            &mut self.cache,
            Some(&context),
        );
        self.cache.finish();
        self.sheets.retain(|text, _| Rc::strong_count(text) > 1);
        for (&host, &root) in page.shadows {
            result
                .children
                .insert(host, doc.nodes[root].children.clone());
        }
        for (&slot, assigned) in &shadow_walk.slots {
            if !assigned.is_empty() {
                result.children.insert(slot, assigned.clone());
            }
        }
        let serial = self.walk.serial;
        let mut relisted = false;
        if self
            .walk
            .update(doc, &result.children, self.cache.document_walk())
        {
            let inert = |id: usize| doc.nodes[id].attrs.contains_key("inert");
            let editable = |id: usize| {
                let node = &doc.nodes[id];
                matches!(node.tag.as_str(), "input" | "textarea")
                    || node.content_editable() == Some(true)
            };
            // The marks of the last pass tell which nodes the lists hold.
            let kept = self.walk.since(serial).is_some_and(|touched| {
                touched.iter().all(|&id| {
                    let id = id as usize;
                    self.select_marks
                        .get(id)
                        .is_none_or(|marks| marks & (EDITABLE | OWN_INERT) == 0)
                        && !(self.walk.contains(id) && (inert(id) || editable(id)))
                })
            });
            if !kept || cfg!(debug_assertions) {
                let nodes = self.walk.order().iter();
                let nodes = nodes.map(|&(id, _)| id as usize);
                let lists = (
                    nodes.clone().filter(|&id| editable(id)).collect::<Vec<_>>(),
                    nodes.filter(|&id| inert(id)).collect::<Vec<_>>(),
                );
                relisted = (&lists.0, &lists.1) != (&self.editable, &self.inert);
                assert!(!kept || !relisted, "kept editable and inert lists differ");
                (self.editable, self.inert) = lists;
            }
        }
        let subject = [crate::dom::MODAL, crate::dom::FULLSCREEN]
            .into_iter()
            .find_map(|bit| {
                page.top_layer
                    .iter()
                    .rev()
                    .copied()
                    .find(|&id| doc.nodes.get(id).is_some_and(|node| node.state & bit != 0))
            });
        let walk = &self.walk;
        // A node takes its marks and selection mode from its parent. Returns whether they changed.
        let settle = |marks: &mut [u8],
                      select: &mut super::Chunked<(super::UserSelect, usize)>,
                      id: usize,
                      parent: u32| {
            let (inherited, hidden, inert) = match parent {
                u32::MAX => ((super::UserSelect::Text, 0), false, false),
                parent => (
                    select[parent as usize],
                    marks[parent as usize] & HIDDEN != 0,
                    Some(id) != subject
                        && (parent == 0 && subject.is_some()
                            || marks[parent as usize] & INERT != 0),
                ),
            };
            let flags = result.flags[id];
            let hidden = hidden || flags & super::DISPLAY_NONE != 0;
            let inert = inert || marks[id] & OWN_INERT != 0;
            let own = marks[id] & (EDITABLE | OWN_INERT);
            let mode = if hidden || flags & super::INVISIBLE != 0 {
                super::UserSelect::None
            } else if own & EDITABLE != 0 {
                super::UserSelect::Text
            } else if flags & super::SELECT_NONE != 0 {
                super::UserSelect::None
            } else if flags & super::SELECT_ALL != 0 {
                super::UserSelect::All
            } else if flags & super::SELECT_TEXT != 0 {
                super::UserSelect::Text
            } else {
                inherited.0
            };
            let new = (
                own | (u8::from(hidden) * HIDDEN) | (u8::from(inert) * INERT),
                (mode, if mode == inherited.0 { inherited.1 } else { id }),
            );
            let changed = (marks[id], select[id]) != new;
            if changed {
                (marks[id], select[id]) = new;
            }
            changed
        };
        let all = |marks: &mut Vec<u8>, select: &mut super::Chunked<(super::UserSelect, usize)>| {
            marks.clear();
            marks.resize(doc.nodes.len(), 0);
            for &id in &self.editable {
                marks[id] = EDITABLE;
            }
            for &id in &self.inert {
                marks[id] |= OWN_INERT;
            }
            for &(id, parent) in walk.order().iter() {
                settle(marks, select, id as usize, parent);
            }
        };
        // The last pass gives the marks when it had the same lists, modal subject and, but for a patch, walk.
        // Then only nodes with other style flags or another place can change, and the nodes below them.
        let base = context.previous.as_deref().filter(|previous| {
            self.select_base.0 == std::ptr::from_ref(*previous) as usize
                && self.select_base.1 == subject
                && !relisted
                && self.select_marks.len() <= doc.nodes.len()
        });
        let touched = base.and_then(|_| match self.select_base.2 == walk.serial {
            true => Some(&[][..]),
            false => walk.since(self.select_base.2),
        });
        let mut marks = std::mem::take(&mut self.select_marks);
        let mut select = std::mem::take(&mut result.user_select);
        page.bounds.reserve(doc.nodes.len().saturating_sub(1));
        // `visibility: hidden` also takes focus away, as in browsers.
        let unfocusable = |id: usize, marks: u8| {
            marks & (HIDDEN | INERT) != 0 || result.flags[id] & super::INVISIBLE != 0
        };
        let mirror = |bounds: &LayoutMirror, id: usize, marks: u8| {
            bounds.mark_inert(id, unfocusable(id, marks));
        };
        let mut contents = None;
        match (base, touched) {
            (Some(previous), Some(touched)) => {
                select.clone_from(&previous.user_select);
                select.resize(doc.nodes.len(), (super::UserSelect::None, 0));
                marks.resize(doc.nodes.len(), 0);
                let mut places = Vec::new();
                for id in result.flags.differing(&previous.flags).flatten() {
                    if previous.flags.get(id) != Some(&result.flags[id])
                        && let Some(at) = walk.place(id)
                    {
                        places.push(at as usize);
                    }
                }
                for &id in touched {
                    match walk.place(id as usize) {
                        Some(at) => places.push(at as usize),
                        None => {
                            marks[id as usize] = 0;
                            select[id as usize] = (super::UserSelect::None, 0);
                        }
                    }
                }
                places.sort_unstable();
                let mut flattened = self.select_base.3;
                let (mut next, mut scan) = (0, 0);
                let mut places = places.into_iter();
                loop {
                    if next >= scan {
                        match places.find(|&at| at >= next) {
                            Some(at) => next = at,
                            None => break,
                        }
                    }
                    let (id, parent) = walk.order()[next];
                    if settle(&mut marks, &mut select, id as usize, parent) {
                        scan = scan.max(next + walk.span(id as usize));
                    }
                    mirror(page.bounds, id as usize, marks[id as usize]);
                    flattened |= result.flags[id as usize] & super::DISPLAY_CONTENTS != 0;
                    next += 1;
                }
                // A node that stops to pass its children on needs the scan below, like every pass after one.
                contents = Some(flattened);
                #[cfg(debug_assertions)]
                {
                    let (mut full, mut modes) = (Vec::new(), select.clone());
                    all(&mut full, &mut modes);
                    assert!(
                        full == marks && modes == select,
                        "kept selection marks differ"
                    );
                }
            }
            _ => {
                select.clear();
                select.resize(doc.nodes.len(), (super::UserSelect::None, 0));
                all(&mut marks, &mut select);
                for &(id, _) in walk.order().iter() {
                    mirror(page.bounds, id as usize, marks[id as usize]);
                }
            }
        }
        #[cfg(debug_assertions)]
        for &(id, _) in walk.order().iter() {
            let shown = !unfocusable(id as usize, marks[id as usize]);
            let mirrored = !page.bounds.inert(id as usize);
            assert!(shown == mirrored, "kept inert mirror differs");
        }
        let mut flatten = contents.unwrap_or_else(|| {
            let mut nodes = walk.order().iter();
            nodes.any(|&(id, _)| result.flags[id as usize] & super::DISPLAY_CONTENTS != 0)
        });
        #[cfg(debug_assertions)]
        {
            let mut nodes = walk.order().iter();
            let any =
                nodes.any(|&(id, _)| result.flags[id as usize] & super::DISPLAY_CONTENTS != 0);
            assert!(flatten || !any, "kept display contents scan differs");
        }
        (self.select_marks, result.user_select) = (marks, select);
        let serial = walk.serial;
        if flatten {
            let contents = |child: usize| result.flags[child] & super::DISPLAY_CONTENTS != 0;
            let mut parents = vec![false; doc.nodes.len()];
            flatten = false;
            for &(id, parent) in self.walk.order().iter() {
                if contents(id as usize) {
                    flatten = true;
                    if parent != u32::MAX {
                        parents[parent as usize] = true;
                    }
                }
            }
            for &(id, _) in self.walk.order().iter().rev() {
                let id = id as usize;
                if parents[id] {
                    let children = result.children.get(&id).unwrap_or(&doc.nodes[id].children);
                    let mut flattened = Vec::new();
                    for &child in children {
                        if contents(child) {
                            flattened.extend(
                                result
                                    .children
                                    .get(&child)
                                    .unwrap_or(&doc.nodes[child].children),
                            );
                        } else {
                            flattened.push(child);
                        }
                    }
                    result.children.insert(id, flattened);
                }
            }
        }
        if let Some(previous) = &context.previous {
            for range in result.values.differing(&previous.values) {
                for id in range {
                    let new = &result.values[id];
                    if previous.values.get(id).is_some_and(|old| {
                        !Rc::ptr_eq(old, new) && old.differs_in(new, &ANCHORING_PROPS)
                    }) && self.unanchored.len() < 4096
                    {
                        self.unanchored.push(id);
                    }
                }
            }
        }
        page.bounds.zoomed(result.lengths.zoomed);
        if animating {
            let result = Rc::new(result);
            self.select_base = (Rc::as_ptr(&result) as usize, subject, serial, flatten);
            return result;
        }
        self.animating.clear();
        self.animating_pseudos.clear();
        self.animating_units.clear();
        self.epoch += 1;
        page.bounds.restyled();
        self.key = key;
        let result = Rc::new(result);
        self.select_base = (Rc::as_ptr(&result) as usize, subject, serial, flatten);
        self.linted = false;
        self.computed = Some(result.clone());
        self.previous = None;
        result
    }

    pub(crate) fn inert(&self, id: usize) -> bool {
        self.select_marks
            .get(id)
            .is_some_and(|marks| marks & INERT != 0)
    }
}

// Like Chrome, a change of these properties of the scroll anchor or a box around it keeps the scroll offset.
const ANCHORING_PROPS: crate::css::properties::PropSet = crate::css::properties::PropSet::new(&[
    Prop::POSITION,
    Prop::TOP,
    Prop::RIGHT,
    Prop::BOTTOM,
    Prop::LEFT,
    Prop::INSET_BLOCK_START,
    Prop::INSET_BLOCK_END,
    Prop::INSET_INLINE_START,
    Prop::INSET_INLINE_END,
    Prop::WIDTH,
    Prop::HEIGHT,
    Prop::MIN_WIDTH,
    Prop::MIN_HEIGHT,
    Prop::MAX_WIDTH,
    Prop::MAX_HEIGHT,
    Prop::INLINE_SIZE,
    Prop::BLOCK_SIZE,
    Prop::MIN_INLINE_SIZE,
    Prop::MIN_BLOCK_SIZE,
    Prop::MAX_INLINE_SIZE,
    Prop::MAX_BLOCK_SIZE,
    Prop::MARGIN_TOP,
    Prop::MARGIN_RIGHT,
    Prop::MARGIN_BOTTOM,
    Prop::MARGIN_LEFT,
    Prop::PADDING_TOP,
    Prop::PADDING_RIGHT,
    Prop::PADDING_BOTTOM,
    Prop::PADDING_LEFT,
    Prop::TRANSFORM,
    Prop::TRANSLATE,
    Prop::ROTATE,
    Prop::SCALE,
    Prop::MARGIN_BLOCK_START,
    Prop::MARGIN_BLOCK_END,
    Prop::MARGIN_INLINE_START,
    Prop::MARGIN_INLINE_END,
    Prop::PADDING_BLOCK_START,
    Prop::PADDING_BLOCK_END,
    Prop::PADDING_INLINE_START,
    Prop::PADDING_INLINE_END,
]);
