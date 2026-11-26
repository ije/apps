use super::super::Build;
use super::super::kept::{Query, hashed};
use super::super::margin::{edge, float_side, run_cuts};
use super::super::plan::Plan;
use crate::css::properties::{Prop, Style};
use crate::css::value::units::Lengths;
use crate::dom::Node;
use crate::render::inline::InlineText;
use crate::render::layout::cull;
use crate::render::layout::size::aspect_ratio;
use crate::render::paint::text::AtomSlot;
use crate::style;
use renderer::{point, px, relative};
use std::{cell::RefCell, rc::Rc};

/// The facts of one box that the stages of `create` share.
pub(super) struct Cx<'a> {
    /// The build of the parent, which logs the reads of this box.
    pub(super) reader: &'a Build<'a>,
    /// The build of the children of this box.
    pub(super) build: &'a Build<'a>,
    pub(super) id: usize,
    pub(super) node: &'a Node,
    pub(super) style: &'a Rc<Style>,
    pub(super) units: Lengths,
    pub(super) parsed: &'a Rc<Plan>,
    pub(super) display: &'a str,
    pub(super) position: &'a str,
    pub(super) containing: [f32; 2],
    pub(super) container: [f32; 2],
    pub(super) slot: &'a Option<([f32; 4], crate::render::layout::grid::Grid)>,
    pub(super) pseudo: Option<usize>,
    pub(super) depth: usize,
    pub(super) multicol: bool,
    pub(super) flow: bool,
    pub(super) flat: &'a [usize],
    pub(super) cuts: &'a [(usize, usize, usize)],
    pub(super) cut_ids: &'a rustc_hash::FxHashSet<usize>,
    pub(super) cut_pseudos: &'a [(usize, usize)],
    pub(super) flexbox: bool,
    pub(super) editable: bool,
    pub(super) stacking: bool,
    pub(super) composited: bool,
    pub(super) drawable: bool,
    pub(super) layered: bool,
    /// The length of the read log when the build of the box started.
    pub(super) mark: usize,
    pub(super) order: Option<i32>,
    pub(super) positioned: bool,
    pub(super) joins: bool,
    pub(super) parent_box: Option<usize>,
    pub(super) positioner: Option<usize>,
    /// The key of the boxes that can share the style of this one.
    pub(super) share: Option<u64>,
    pub(super) parent: Option<usize>,
    pub(super) block_size: Option<[f32; 2]>,
    pub(super) opacity: f32,
    pub(super) effect_layer: Option<f32>,
    pub(super) selection_colors: (Option<u32>, u32),
    pub(super) broken: bool,
    pub(super) alt: Option<&'a String>,
    pub(super) sizeless: bool,
    pub(super) first_content: Option<usize>,
    pub(super) last_content: Option<usize>,
    pub(super) run_texts: RefCell<rustc_hash::FxHashMap<usize, Option<Rc<InlineText>>>>,
    pub(super) layers: RefCell<crate::render::inline::Layers>,
    pub(super) rtl: bool,
    pub(super) line_justify: Option<renderer::AlignContent>,
    pub(super) line_align: Option<renderer::AlignItems>,
    pub(super) all_inline: bool,
    /// The style of an equal box built before, which this box shares.
    pub(super) hit: Option<Rc<renderer::Style>>,
}

/// The boxes and line state that a stage leaves for the stages after it.
#[derive(Default)]
pub(super) struct Carry {
    pub(super) after: Vec<(i32, renderer::Node)>,
    // Inline pseudo-elements of a block that also holds blocks, which join the first and last lines.
    pub(super) lines: [Option<renderer::Node>; 2],
    // The placeholder of an out-of-flow `::after` at its static place, at the end of the last line.
    pub(super) static_anchor: Option<renderer::Node>,
    // In-flow pseudo-elements of a grid or flex box are its first and last items, which the grid places and
    // flex orders among the children.
    pub(super) items: [Option<(i32, renderer::Node, [f32; 4])>; 2],
    // Pseudo-elements that are atoms of the element's text, built here and placed with the text below.
    pub(super) pseudo_atoms: [Option<renderer::Node>; 2],
    pub(super) pseudo_origin: Option<Rc<std::cell::Cell<renderer::Point<renderer::Pixels>>>>,
    pub(super) strut_due: bool,
    // Each line has the strut: each box of a line of boxes carries one, or the space that it joins.
    pub(super) struts: bool,
    pub(super) strut_next: bool,
}

pub(super) fn percentage(value: &str) -> Option<f32> {
    value
        .strip_suffix('%')
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
}

/// A box that lays out in a frame turned by 90 degrees: the size of the frame along and across its lines.
pub(super) struct Turn {
    pub(super) clockwise: bool,
    pub(super) inline: f32,
    pub(super) block: f32,
}

/// The edges and sizes of the box, for the stages after its padding, borders and margins are known.
pub(super) struct Frame {
    pub(super) padding: [f32; 4],
    pub(super) borders: [f32; 4],
    pub(super) margin: [f32; 4],
    pub(super) extra: [f32; 2],
    pub(super) intrinsic_width: Option<f32>,
    pub(super) turn: Option<Turn>,
    pub(super) inset_x: [Option<f32>; 2],
    pub(super) natural_ratio: Option<f32>,
}

impl Frame {
    /// The automatic minimum size of a flex item on its main axis: `size`, cut to its max size.
    pub(super) fn floor(&self, cx: &Cx<'_>, axis: usize, size: f32) -> Option<f32> {
        let Cx {
            reader,
            id,
            style,
            units,
            containing,
            ..
        } = *cx;
        let (flex_parent, extra) = (reader.flex_parent, self.extra);
        let [min, max] = [
            [Prop::MIN_WIDTH, Prop::MAX_WIDTH],
            [Prop::MIN_HEIGHT, Prop::MAX_HEIGHT],
        ][axis];
        if !flex_parent
            || style.get(min).is_some_and(|value| value != "auto")
            || reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                != (axis == 1)
        {
            return None;
        }
        match style.get(max).filter(|value| *value != "none") {
            Some(value) if value.contains('%') => None,
            Some(value) => units
                .resolve(value, containing[axis])
                .map(|max| size.min(max + extra[axis])),
            None => Some(size),
        }
    }

    pub(super) fn content_box(&self, cx: &Cx<'_>) -> [f32; 2] {
        let Frame {
            padding,
            borders,
            margin,
            extra,
            intrinsic_width,
            inset_x,
            natural_ratio,
            ..
        } = *self;
        let Cx {
            reader,
            style,
            units,
            containing,
            slot,
            depth,
            ..
        } = *cx;
        if let Some(turn) = &self.turn {
            return [
                turn.inline - padding[1] - padding[3] - borders[1] - borders[3],
                turn.block - padding[0] - padding[2] - borders[0] - borders[2],
            ];
        }
        let mut content = containing;
        for (axis, (prop, min, max)) in [
            (Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH),
            (Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
                .map(|value| value + extra[axis])
                .or(intrinsic_width.filter(|_| axis == 0))
            {
                content[axis] = value;
            } else if axis == 0 {
                content[axis] -= margin[1] + margin[3];
                content[axis] -= inset_x.iter().flatten().sum::<f32>();
            } else if let Some(ratio) = style
                .get(Prop::ASPECT_RATIO)
                .filter(|value| !(value.contains("auto") && natural_ratio.is_some()))
                .and_then(aspect_ratio)
            {
                // Like the layout, the ratio applies to the border box.
                content[1] =
                    (content[0] + padding[1] + padding[3] + borders[1] + borders[3]) / ratio;
            }
            if let Some(value) = style
                .get(max)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].min(value + extra[axis]);
            }
            if let Some(value) = style
                .get(min)
                .and_then(|v| units.resolve(v, containing[axis]))
            {
                content[axis] = content[axis].max(value + extra[axis]);
            }
            content[axis] = (content[axis]
                - padding[1 - axis]
                - padding[3 - axis]
                - borders[1 - axis]
                - borders[3 - axis])
                .max(0.);
        }
        if let Some((bounds, _)) = &slot {
            reader.log(
                Query::Cell,
                depth,
                hashed(Some([bounds[2], bounds[3]].map(f32::to_bits))),
            );
            content[0] = (bounds[2]
                - padding[1]
                - padding[3]
                - margin[1]
                - margin[3]
                - borders[1]
                - borders[3])
                .max(0.);
            content[1] = (bounds[3]
                - padding[0]
                - padding[2]
                - margin[0]
                - margin[2]
                - borders[0]
                - borders[2])
                .max(0.);
        }
        content
    }
}

/// The height above the baseline of the first line of a box.
pub(super) fn above(build: &Build<'_>, units: &Lengths, parsed: &Plan) -> f32 {
    let text = build.text;
    f32::from(text.baseline(
        text.face(&parsed.face),
        px(units.font.em),
        px(parsed.line_height),
        build.pixel_ratio,
    ))
}

// The initial containing block: the viewport without the scroll bars of the page.
pub(super) fn viewport(reader: &Build<'_>) -> [f32; 2] {
    reader.log(Query::Viewport, 0, hashed(reader.port.map(f32::to_bits)));
    reader.port
}

// Makes a wrapper as wide as its line, so the percent width of the box in it takes the line. Negative
// margins cut the wrapper back to the box, on the sides where the line has its free space.
pub(super) fn spread(
    style: &mut renderer::Style,
    (fraction, offset): (f32, renderer::Pixels),
    justify: Option<renderer::AlignContent>,
) {
    let spare = |margin: renderer::Length, share: f32| {
        let own = match margin {
            renderer::Length::Px(own) => own,
            _ => px(0.),
        };
        renderer::Length::Calc((fraction - 1.) * share, offset * share + own)
    };
    (style.size.width, style.flex_shrink) = (relative(1.), 0.);
    style.justify_content = justify;
    match justify {
        Some(renderer::AlignContent::Center) => {
            style.margin.left = spare(style.margin.left, 0.5);
            style.margin.right = spare(style.margin.right, 0.5);
        }
        Some(_) => style.margin.left = spare(style.margin.left, 1.),
        None => style.margin.right = spare(style.margin.right, 1.),
    }
}

impl Cx<'_> {
    pub(super) fn strut(&self) -> renderer::Node {
        let above = above(self.build, &self.units, self.parsed);
        renderer::node()
            .w(px(0.))
            .h(px(above))
            .mb(px(self.parsed.line_height - above))
            .flex_shrink_0()
            .baseline(renderer::Baseline::Bottom)
    }

    pub(super) fn align_of(&self, element: &mut renderer::Child) -> Option<renderer::AlignItems> {
        match element {
            renderer::Child::Node(node) => node.style_ref().align_self,
            renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => self
                .build
                .tree
                .style_of(*id)
                .and_then(|style| style.align_self),
        }
    }

    // The width of an inline-level box as a share of its line plus a length, with its side margins. `None`
    // inside: the box has percent sizes that are no such share, like a percent width with a fixed limit.
    // A wrapper that shrinks to the box gives these percentages nothing to resolve against.
    pub(super) fn percent_width(
        &self,
        element: &renderer::Child,
    ) -> Option<Option<(f32, renderer::Pixels)>> {
        let build = self.build;
        let style = match element {
            renderer::Child::Node(node) => node.style_ref(),
            renderer::Child::Kept(id) | renderer::Child::Placed(id, _) => {
                build.tree.style_of(*id)?
            }
        };
        // CSS percentages are `Calc` lengths, or bounds that are no such share. A plain fraction is a limit that
        // the box builder sets itself.
        let share = |length| match length {
            renderer::Length::Calc(fraction, offset) => Some((fraction, offset)),
            _ => None,
        };
        let limits = [style.min_size.width, style.max_size.width];
        if [style.size.width, limits[0], limits[1]]
            .iter()
            .any(|length| matches!(length, renderer::Length::Bound(_)))
        {
            return Some(None);
        }
        let Some((mut fraction, mut offset)) = share(style.size.width) else {
            return limits
                .iter()
                .any(|&limit| share(limit).is_some())
                .then_some(None);
        };
        // A limit changes the share only when it is a percentage of the same box.
        for (limit, max) in limits.into_iter().zip([false, true]) {
            match share(limit) {
                Some((limit, same)) if same == offset => {
                    fraction = if max {
                        fraction.min(limit)
                    } else {
                        fraction.max(limit)
                    }
                }
                None if !matches!(limit, renderer::Length::Px(_)) => {}
                _ => return Some(None),
            }
        }
        for margin in [style.margin.left, style.margin.right] {
            match (margin, share(margin)) {
                (_, Some((share, length))) => {
                    (fraction, offset) = (fraction + share, offset + length)
                }
                (renderer::Length::Px(length), _) => offset += length,
                _ => {}
            }
        }
        Some(Some((fraction, offset)))
    }

    // Combine the zero-width strut with the first item, or Taffy can wrap the first item after the strut.
    // An item whose percent sizes are no such share of the line stays apart, so they resolve against the line.
    pub(super) fn with_strut(
        &self,
        mut element: renderer::Child,
        justify: Option<renderer::AlignContent>,
    ) -> (renderer::Child, Option<renderer::Child>, bool) {
        let (align_of, percent_width, strut) = (
            |e: &mut renderer::Child| self.align_of(e),
            |e: &renderer::Child| self.percent_width(e),
            || self.strut(),
        );
        let align = align_of(&mut element);
        let percent = percent_width(&element);
        if percent == Some(None) {
            return (strut().into(), Some(element), align.is_some());
        }
        let mut group = renderer::node()
            .flex()
            .flex_row()
            .flex_shrink(1.)
            .child(strut())
            .child(element);
        group.style().align_items = Some(renderer::AlignItems::Baseline);
        group.style().align_self = align;
        group.style().inline = renderer::Inline::Lines;
        if let Some(Some(share)) = percent {
            spread(group.style(), share, justify);
        }
        (renderer::Child::from(group), None, align.is_some())
    }

    pub(super) fn floated(&self, child: usize) -> bool {
        let (styles, flow) = (self.build.styles, self.flow);
        flow && float_side(&styles.values[child]).is_some()
            && styles.values[child]
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
    }

    pub(super) fn inline_level(&self, child: usize) -> bool {
        let Build { doc, styles, .. } = *self.build;
        let floated = |child| self.floated(child);
        doc.nodes[child].tag.starts_with('#')
            || doc.nodes[child].tag == "br"
            || !floated(child)
                && styles.values[child]
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value.starts_with("inline"))
                && !crate::render::inline::holds_block_box(doc, styles, child)
            || styles.values[child]
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    }

    pub(super) fn inline_text(&self) -> Option<Rc<InlineText>> {
        let Build {
            doc,
            styles,
            measured,
            ..
        } = *self.build;
        let id = self.id;
        measured
            .borrow_mut()
            .texts
            .entry(id)
            .or_insert_with(|| {
                crate::render::inline::inline_text(doc, styles, id, &[], &[], [true; 2])
                    .map(Rc::new)
            })
            .clone()
    }

    // Inline content between the blocks of a box, which lays out as one paragraph.
    pub(super) fn in_run(&self, child: usize) -> bool {
        let Build { doc, styles, .. } = *self.build;
        let Self {
            id, cuts, cut_ids, ..
        } = *self;
        let (inline_level, floated) = (
            |child| self.inline_level(child),
            |child| self.floated(child),
        );
        let node = &doc.nodes[child];
        match node.tag.as_str() {
            "#text" | "#comment" => true,
            _ => {
                let own = &styles.values[child];
                own.get(Prop::DISPLAY) == Some("none")
                    || inline_level(child)
                        && !floated(child)
                        && own
                            .get(Prop::POSITION)
                            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
                    // The text marks the static place of an out-of-flow box.
                    || !self.multicol
                        && crate::render::inline::static_place(own)
                        && !crate::render::inline::layered_after(
                            doc,
                            styles,
                            id,
                            child,
                            &mut self.layers.borrow_mut(),
                        )
                    // A float in a spliced box goes beside the line that holds its place.
                    || floated(child)
                        && !cuts.is_empty()
                        && std::iter::successors(node.parent, |&up| doc.nodes[up].parent)
                            .take_while(|&up| up != id)
                            .any(|up| cut_ids.contains(&up))
            }
        }
    }

    pub(super) fn content(&self, child: usize) -> bool {
        let info = self.build.info;
        self.in_run(child) && !crate::render::inline::static_place(&self.build.styles.values[child])
            || !self.floated(child) && info[child].flags & cull::OUT_OF_FLOW == 0
    }

    // The text of the run of `flat` from `index` to `end`, which the first and last runs share with the
    // pseudo-elements of the box. Only boxes with spliced boxes in them hold those in their runs.
    pub(super) fn run_text(
        &self,
        index: usize,
        end: usize,
    ) -> (Option<Rc<InlineText>>, Vec<(usize, u8)>) {
        let Build { doc, styles, .. } = *self.build;
        let Self {
            build,
            id,
            flat,
            cuts,
            first_content,
            last_content,
            ref run_texts,
            ..
        } = *self;
        let in_run = |child| self.in_run(child);
        let run = &flat[index..end];
        let parts = run_cuts(build, cuts, flat, [index, end], in_run);
        let found = run_texts
            .borrow_mut()
            .entry(index)
            .or_insert_with(|| {
                let ends = [
                    first_content.is_some_and(|at| (index..end).contains(&at)),
                    last_content.is_some_and(|at| (index..end).contains(&at)),
                ];
                (run.is_empty()
                    || run.iter().any(|&kid| {
                        doc.nodes[kid].tag == "#text"
                            && !doc.nodes[kid].text.trim_ascii().is_empty()
                    })
                    // The edges of a spliced box make a line also with only white space.
                    || parts.iter().any(|&(cut, flags)| {
                        flags & crate::render::inline::CUT_FIRST != 0 && edge(build, cut, 3)
                            || flags & crate::render::inline::CUT_LAST != 0 && edge(build, cut, 1)
                    }))
                .then(|| crate::render::inline::inline_text(doc, styles, id, run, &parts, ends))
                .flatten()
                .map(|mut data| {
                    // Far above node ids, and unique per parent, so these lines keep their keys.
                    if run.is_empty() {
                        data.key = !(id << 20 | index);
                    }
                    Rc::new(data)
                })
            })
            .clone();
        (found, parts)
    }

    // The runs that start and end the content of a box with spliced boxes.
    pub(super) fn edge_text(&self, index: usize) -> Option<Rc<InlineText>> {
        let Self {
            flat,
            cuts,
            first_content,
            last_content,
            ..
        } = *self;
        let (in_run, run_text) = (
            |child| self.in_run(child),
            |index, end| self.run_text(index, end),
        );
        let at = [first_content, last_content][index]?;
        if cuts.is_empty() || !in_run(flat[at]) {
            return None;
        }
        let [mut start, mut end] = [at, at + 1];
        while start > 0 && in_run(flat[start - 1]) {
            start -= 1;
        }
        while end < flat.len() && in_run(flat[end]) {
            end += 1;
        }
        run_text(start, end).0
    }

    // An inline box with only blank text or comments, which can make a line of its own.
    pub(super) fn empty_inline(&self) -> bool {
        let Build { doc, styles, .. } = *self.build;
        let Self { display, node, .. } = *self;
        display == "inline"
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag.starts_with('#')
                    && crate::render::inline::makes_no_line(doc, styles, child)
            })
    }

    // A box whose children make no line, like spaces and hidden elements, has no baseline of its own.
    pub(super) fn blank(&self) -> bool {
        let Build { doc, styles, .. } = *self.build;
        let node = self.node;
        node.children.iter().all(|&child| {
            let kid = &doc.nodes[child];
            match kid.tag.as_str() {
                "#text" => kid.text.trim_ascii().is_empty(),
                "#comment" => true,
                _ => styles.values[child].get(Prop::DISPLAY) == Some("none"),
            }
        })
    }

    // The size of an inline box in a text as of the last frame, or a guess before it has one.
    pub(super) fn atom_slot(
        &self,
        atom: &crate::render::inline::Atom,
        child: &mut renderer::Child,
        containing: f32,
        pseudo: &dyn Fn() -> [f32; 3],
    ) -> AtomSlot {
        let Build {
            doc,
            styles,
            text,
            view,
            measured,
            ..
        } = *self.build;
        let reader = self.reader;
        let last = view
            .store
            .borrow()
            .page_state(view.id)
            .and_then(|dom| dom.layout.atoms.get(&atom.key()).copied());
        reader.log(
            Query::Atom(atom.key() as u32),
            0,
            hashed(last.map(|size| size.map(f32::to_bits))),
        );
        let size = last.unwrap_or_else(|| {
            if atom.pseudo != 0 {
                return pseudo();
            }
            let [_, max, _, height] = crate::render::layout::grid::intrinsic::intrinsic(
                doc,
                atom.node,
                styles,
                Some(containing),
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let line = styles.lengths[atom.node].font.lh;
            [max, height.max(line), line * 0.8]
        });
        crate::render::paint::text::AtomSlot {
            size,
            align: self.align_of(child),
            at: std::cell::Cell::new(point(px(0.), px(0.))),
        }
    }

    // The box that places an inline box where its text puts it, from the padding box of the text's box.
    pub(super) fn atom_box(
        &self,
        index: usize,
        atom: &crate::render::inline::Atom,
        child: renderer::Child,
        slots: &Rc<[AtomSlot]>,
        padding: [f32; 4],
    ) -> renderer::Node {
        let Build { styles, view, .. } = *self.build;
        let id = self.id;
        let mut wrapper = renderer::node()
            .absolute()
            .left(px(padding[3]))
            .top(px(padding[0]))
            .flex()
            .flex_row();
        wrapper.style().align_items = Some(renderer::AlignItems::Baseline);
        // A box that shrinks to its atom gives percent sizes of the atom nothing to resolve against.
        let atom_style = match atom.pseudo {
            0 => &*styles.values[atom.node],
            pseudo => &styles.generated(id).unwrap()[usize::from(pseudo - 1)],
        };
        let rest = [Prop::WIDTH, Prop::MIN_WIDTH, Prop::MAX_WIDTH]
            .iter()
            .any(|&prop| {
                atom_style
                    .get(prop)
                    .is_some_and(|value| value.contains('%'))
            })
            .then(Rc::<std::cell::Cell<renderer::Pixels>>::default);
        wrapper = match &rest {
            Some(_) => wrapper.right(px(padding[1])),
            None => wrapper.max_w(renderer::Length::Calc(1., px(-padding[1] - padding[3]))),
        };
        wrapper = wrapper
            .hooks(crate::render::paint::text::AtomBox {
                slots: slots.clone(),
                index,
                node: atom.key(),
                view: view.clone(),
                rest: rest.clone(),
            })
            .child(child);
        if let Some(rest) = rest {
            wrapper = wrapper.child(
                renderer::node()
                    .flex_1()
                    .min_w(px(0.))
                    .hooks(crate::render::paint::text::AtomRest(rest)),
            );
        }
        wrapper
    }

    /// Whether an `auto` width fills the containing block.
    pub(super) fn fills(&self) -> bool {
        !self.reader.flex_parent
            && !matches!(self.position, "absolute" | "fixed")
            && !self.display.starts_with("inline")
            && (self.style.get(Prop::FLOAT)).is_none_or(|value| value == "none")
    }

    pub(super) fn stretched(&self, axis: usize) -> bool {
        (self.style)
            .get([Prop::WIDTH, Prop::HEIGHT][axis])
            .is_none_or(|value| value == "auto")
            && self.stretches(axis)
    }

    /// Whether the container stretches this flex item when its size in the axis is `auto`.
    pub(super) fn stretches(&self, axis: usize) -> bool {
        let Self {
            reader,
            id,
            position,
            style,
            ..
        } = *self;
        let flex_parent = reader.flex_parent;
        let [start, end] = [
            [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis];
        flex_parent
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                == (axis == 0)
            && !matches!(position, "absolute" | "fixed")
            && [start, end]
                .iter()
                .all(|&prop| style.get(prop) != Some("auto"))
            && style
                .get(Prop::ALIGN_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
    }

    pub(super) fn dark(&self) -> bool {
        let Self { reader, style, .. } = *self;
        if style
            .get(Prop::COLOR_SCHEME)
            .is_some_and(|value| value.contains("dark") && value.contains("light"))
        {
            reader.log(Query::Theme, 0, hashed(crate::native::theme_state() & 1));
        }
        style::dark(style, crate::native::theme_state())
    }
}
