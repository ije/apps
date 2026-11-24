use super::super::paint::text::InlineText;
use super::columns::{column_set, columns};
use super::create::create;
use super::cx::{Carry, Cx, Frame};
use super::margin::inline_fade;
use super::query::static_mark;
use super::{Build, element};
use crate::css::properties::Prop;
use renderer::{px, relative};
use std::rc::Rc;

/// Lays out the inline content of a box as one paragraph, with its floats, atoms and out-of-flow boxes.
pub(super) fn paragraph(
    cx: &Cx<'_>,
    fr: &Frame,
    mut el: renderer::Node,
    carry: &mut Carry,
    data: Rc<crate::render::inline::InlineText>,
    content: Option<renderer::AlignContent>,
) -> renderer::Node {
    let Cx {
        reader,
        build,
        id,
        style,
        units,
        parsed,
        display,
        position,
        containing,
        container,
        slot,
        flow,
        multicol,
        flat,
        line_justify,
        rtl,
        ref hit,
        stacking,
        ..
    } = *cx;
    let Build {
        doc,
        pixel_ratio,
        styles,
        text,
        top_layer,
        view,
        flex_parent,
        ..
    } = *reader;
    let Carry {
        after,
        pseudo_atoms,
        ..
    } = carry;
    let padding = fr.padding;
    let content_box = || fr.content_box(cx);
    let made = parsed.made;
    #[cfg(debug_assertions)]
    reader.extra.set(super::kept::hashed(&*data.text));
    let atom_containing = content_box();
    // An absolute box at its static place in a line has an empty atom there, which gives it its place.
    let marker = |atom: &crate::render::inline::Atom| {
        crate::render::inline::static_inline(match atom.pseudo {
            0 => &styles.values[atom.node],
            pseudo => &styles.pseudos[&id][usize::from(pseudo - 1)],
        })
    };
    let mut atom_elements: Vec<renderer::Child> = data
        .atoms
        .iter()
        .map(|atom| match atom.pseudo {
            _ if marker(atom) => static_mark(view, atom.key()).into(),
            0 => element(build, atom.node, atom_containing, container, None),
            pseudo => pseudo_atoms[usize::from(pseudo - 1)]
                .take()
                .unwrap_or_default()
                .into(),
        })
        .collect();
    let atom_slots: Rc<[super::super::paint::text::AtomSlot]> = data
        .atoms
        .iter()
        .zip(&mut atom_elements)
        .map(|(atom, child)| {
            if marker(atom) {
                // At the top of its line, as Chrome places the box.
                return super::super::paint::text::AtomSlot {
                    size: [0.; 3],
                    align: Some(renderer::AlignItems::FlexStart),
                    at: Default::default(),
                };
            }
            cx.atom_slot(atom, child, containing[0], &|| {
                super::super::layout::grid::intrinsic::pseudo_atom(
                    doc,
                    id,
                    usize::from(atom.pseudo - 1),
                    styles,
                    content_box()[0],
                    Some(containing[0]),
                    text,
                    pixel_ratio,
                )
                .unwrap_or_default()
            })
        })
        .collect();
    let text_box = style.get(Prop::TEXT_BOX).unwrap_or("");
    let trim = style.get(Prop::TEXT_BOX_TRIM).unwrap_or(text_box);
    let edge = style.get(Prop::TEXT_BOX_EDGE).unwrap_or(text_box);
    let (over, under) = (
        trim.contains("trim-both") || trim.contains("trim-start"),
        trim.contains("trim-both") || trim.contains("trim-end"),
    );
    let (mut trim_top, mut trim_bottom) = (0., 0.);
    if over || under {
        let system = build.text;
        let font = system.face(&parsed.face);
        let (size, height) = (px(units.font.em), px(units.font.lh));
        let baseline = f32::from(system.baseline(font, size, height, build.pixel_ratio));
        let [ascent, descent, _] = system.line_metrics(font, size).map(f32::from);
        if over {
            trim_top = baseline
                - if edge.contains("cap") {
                    units.font.cap
                } else if edge.contains("ex") {
                    units.font.ex
                } else {
                    ascent
                };
        }
        if under {
            trim_bottom = units.font.lh
                - baseline
                - if edge.contains("alphabetic") {
                    0.
                } else {
                    descent
                };
        }
    }
    // The floats of the paragraph come first, so layout places them before its lines. Out-of-flow boxes
    // that the text leaves out come after it.
    let mut placed = Vec::new();
    for &child in flat {
        if cx.floated(child) {
            let float = element(build, child, content_box(), container, None);
            el = el.child(float);
        } else if doc.nodes[child].parent == Some(id)
            && styles.values[child].get(Prop::DISPLAY) != Some("none")
            && crate::render::inline::placed_out_of_flow(&styles.values[child])
            && !top_layer.contains(&child)
        {
            placed.push(child);
        }
    }
    if data.greedy && hit.is_none() {
        el.text_style().set_text_wrap(renderer::TextWrap::Wrap);
    }
    let mut paragraph = renderer::Node::from(InlineText {
        data: data.clone(),
        mixed: data.mixed(build.text),
        atoms: atom_slots.clone(),
        view: view.clone(),
        shaped: Default::default(),
        prepainted: Default::default(),
        rtl,
        justify: style
            .get(Prop::TEXT_ALIGN)
            .is_some_and(|value| value == "justify")
            .then(|| {
                style
                    .get(Prop::TEXT_ALIGN_LAST)
                    .is_some_and(|value| value == "justify")
            }),
    });
    let alone = el.kids().is_empty()
        && placed.is_empty()
        && data.lifted.is_empty()
        && data.atoms.is_empty()
        && trim_top == 0.
        && trim_bottom == 0.
        && after.is_empty()
        && !stacking
        && slot.is_none()
        && (flex_parent || display != "inline")
        && !rtl
        && line_justify.is_none()
        && content.is_none()
        && matches!(position, "static" | "relative")
        && el.style_ref().overflow.x == renderer::Overflow::Visible
        && el.style_ref().overflow.y == renderer::Overflow::Visible
        && super::super::effects::TRANSFORM_PROPS
            .iter()
            .all(|&prop| style.get(prop).is_none_or(|value| value == "none"))
        && style
            .get(Prop::WRITING_MODE)
            .is_none_or(|mode| mode.starts_with("horizontal"));
    if !alone {
        paragraph = paragraph.id(("text", data.key));
    }
    paragraph.style().flow = renderer::Flow::BLOCK;
    // Text straight in a multi-column box breaks between its rows.
    let multicol = (flow && data.atoms.is_empty())
        .then(|| {
            let content = content_box();
            columns(style, &units, content[0])
                .filter(|_| multicol)
                .map(|set| (set, content))
        })
        .flatten();
    el = if let Some(((count, gap, rule), content)) = multicol {
        let paragraph = paragraph.min_w(px(0.)).flex_shrink(1.);
        el.child(column_set(
            style,
            content,
            count,
            gap,
            rule,
            true,
            [paragraph],
        ))
    } else if alone {
        el.adopt(paragraph)
    } else {
        el.child(
            paragraph
                .min_w(px(0.))
                .max_w(relative(1.))
                .flex_shrink(1.)
                .mt(px(-trim_top))
                .mb(px(-trim_bottom)),
        )
    };
    for (index, (atom, child)) in data.atoms.iter().zip(atom_elements).enumerate() {
        let atom_box = cx.atom_box(index, atom, child, &atom_slots, padding);
        el = el.child(match inline_fade(doc, styles, atom.node, id) {
            fade if fade < 1. => atom_box.hooks(super::super::paint::Effected {
                effect: renderer::PaintEffect {
                    opacity: fade,
                    ..Default::default()
                },
                backdrop: None,
            }),
            _ => atom_box,
        });
    }
    for child in placed {
        // The containing block of a positioned child is the padding box.
        let content = content_box();
        let containing = match styles.values[child].get(Prop::POSITION) {
            Some("fixed") if !made.fixed => content,
            Some("absolute") if !made.absolute => content,
            _ => [
                content[0] + padding[1] + padding[3],
                content[1] + padding[0] + padding[2],
            ],
        };
        el = el.child(element(build, child, containing, container, None));
    }
    // Out-of-flow boxes inside the inline boxes of the text, and at their static place: they lay out beside it,
    // with the inline boxes around them as ancestors.
    for &(node, pseudo) in &data.lifted {
        if top_layer.contains(&node) {
            continue;
        }
        let path: Vec<usize> =
            std::iter::successors(doc.nodes[node].parent, |&up| doc.nodes[up].parent)
                .take_while(|&up| up != id)
                .collect();
        let lifted = Build {
            ancestors: build
                .ancestors
                .iter()
                .copied()
                .chain(path.into_iter().rev())
                .collect(),
            flex_parent: false,
            perspective: None,
            zone: None,
            #[cfg(debug_assertions)]
            extra: Default::default(),
            #[cfg(debug_assertions)]
            plan: Default::default(),
            ..*build
        };
        let content = content_box();
        let padded = [
            content[0] + padding[1] + padding[3],
            content[1] + padding[0] + padding[2],
        ];
        let containing = if made.absolute { padded } else { content };
        el = el.child(match pseudo {
            0 => element(&lifted, node, containing, container, None),
            pseudo => create(
                &lifted,
                node,
                containing,
                container,
                None,
                Some(usize::from(pseudo - 1)),
            ),
        });
    }
    el
}
