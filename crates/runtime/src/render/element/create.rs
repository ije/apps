use super::super::layout::size::{
    aspect_ratio, broken_image, dimension, field_line, field_size, file_size, layout_ceil, legend,
    natural_size, text_field,
};
use super::super::{
    layout::cull,
    paint::{DomBox, Turned, background::css_background, scrollbar},
};
use super::border::{border_canvas, fieldset_border, inset_shadows, shade_border};
use super::children::children;
use super::columns::columns;
use super::cx::{Carry, Cx, Frame, above, percentage, viewport};
use super::kept::{Query, Read, Replay, Shared, hashed};
use super::margin::{
    body_overflow_moves, clear_bits, definite_line, float_side, in_flow, margins, overscroll,
    percent_bound, percent_calc, shrinks_around,
};
use super::paragraph::paragraph;
use super::pseudos::pseudo_elements;
use super::query::{
    LOOK, backdrop, box_parent, boxless, collapses, content_editable, inset_axes, insets,
    only_auto_ratio, padding_start, parent_font, positioner, row_fill, scrolled, scroller,
    starts_line, text_indent,
};
use super::replaced::replaced;
use super::svg::svg_box;
use super::{Build, element};
use crate::css;
use crate::css::properties::Prop;
use crate::style::{self};
use renderer::{Child, Color, Rect, SharedString, point, px, relative, size};
use std::{cell::RefCell, ops::ControlFlow, rc::Rc};

/// Builds the box of `id`, or with `pseudo` only that block pseudo-element of a spliced inline box.
pub(super) fn create(
    build: &Build<'_>,
    id: usize,
    containing: [f32; 2],
    container: [f32; 2],
    slot: Option<([f32; 4], super::super::layout::grid::Grid)>,
    pseudo: Option<usize>,
) -> Child {
    let reader = build;
    let Build {
        doc,
        styles,
        text,
        pixel_ratio,
        view,
        ref ancestors,
        flex_parent,
        measured,
        perspective: ref parent_perspective,
        siblings,
        ids,
        info,
        ref zone,
        columned,
        top_layer,
        reads,
        ..
    } = *build;
    let depth = ancestors.len();
    let mark = reads.borrow().len();
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    let display =
        style
            .get(Prop::DISPLAY)
            .unwrap_or(if node.tag.starts_with('#') && node.tag != "#text" {
                "block"
            } else {
                "inline"
            });
    // Like Chrome, an inline broken image with alt text ignores its size, also when only the UA sheet made it
    // an inline block.
    let broken = broken_image(node);
    let alt = broken
        .then(|| node.attrs.get("alt").filter(|alt| !alt.is_empty()))
        .flatten();
    let sizeless = alt.is_some()
        && (display == "inline"
            || display == "inline-block" && styles.flags[id] & style::AGENT_DISPLAY != 0);
    let display = if sizeless {
        "inline-block"
    } else if display == "inline" && crate::render::inline::holds_block_box(doc, styles, id) {
        "block"
    } else {
        display
    };
    let mut units = styles.lengths[id];
    units.container = Some(container);
    let parsed = view.plans.borrow_mut().get(style, &units, pixel_ratio);
    let order = parsed.order;
    let positioned = style
        .get(Prop::POSITION)
        .is_some_and(|value| value != "static");
    let layered = order.is_some()
        && (positioned
            || flex_parent
            || matches!(
                reader.parent(id, Prop::DISPLAY),
                Some("grid" | "inline-grid")
            ));
    // Keep one stacking context and layer throughout opacity animations so frames only patch opacity.
    let composited = info[id].flags & cull::COMPOSITED != 0;
    let made = parsed.made;
    // Every layer needs a stacking context so z-indexed descendants cannot escape its transform, opacity or
    // clip.
    let stacking = id == 0 || composited || made.stacking || layered;
    let chain: Rc<[usize]> = ancestors.iter().copied().chain([id]).collect();
    // A column width can give columns for a wider content box than the containing one, so it always counts here.
    let multicol = columns(style, &units, containing[0]).is_some()
        || style
            .get(Prop::COLUMN_WIDTH)
            .is_some_and(|value| value != "auto");
    let child_build = Build {
        columned: columned || multicol,
        #[cfg(debug_assertions)]
        extra: Default::default(),
        #[cfg(debug_assertions)]
        plan: Default::default(),
        // Columns move boxes away from their flow, and a spacer for a box could not break between columns.
        zone: zone
            .as_ref()
            .filter(|_| info[id].flags & (cull::CHANGED | cull::MOVES_CONTENT) == 0)
            .filter(|_| !multicol)
            // The contents of a box that skips them at times keep no spacers: their records could outlive a skip.
            .filter(|_| {
                info[id].flags & cull::SKIPS == 0
                    || style
                        .get(Prop::CONTENT_VISIBILITY)
                        .is_none_or(|value| value == "visible")
            })
            .and_then(|zone| {
                if style
                    .get(Prop::OVERFLOW_Y)
                    .is_none_or(|value| value == "visible")
                {
                    return Some(zone.clone());
                }
                let host = view.host.borrow();
                let dom = host.page_state(view.id)?;
                let rect = *dom.geometry.mirror.get(&id)?;
                let moved = dom.layout.cull.scrolled.get(&id).copied().unwrap_or(0.);
                Some(Rc::new(cull::Zone {
                    top: zone.top.max(rect[1]) + moved,
                    bottom: zone.bottom.min(rect[1] + rect[3]) + moved,
                    clips: zone.clips.iter().copied().chain([id]).collect(),
                    resized: zone.resized,
                }))
            }),
        ancestors: chain.clone(),
        flex_parent: matches!(display, "flex" | "inline-flex"),
        perspective: style
            .get(Prop::PERSPECTIVE)
            .filter(|value| *value != "none")
            .and_then(|value| styles.lengths[id].resolve(value, 0.))
            .map(|distance| {
                (
                    distance,
                    style
                        .get(Prop::PERSPECTIVE_ORIGIN)
                        .map(str::to_owned)
                        .unwrap_or_else(|| "50% 50%".into()),
                )
            }),
        ..*reader
    };
    let build = &child_build;
    let position = style.get(Prop::POSITION).unwrap_or("static");
    let parent = box_parent(doc, styles, id);
    let positioner = positioner(position, ancestors.iter().rev().copied(), |ancestor| {
        super::super::effects::establishes(&styles.values[ancestor])
    });
    if matches!(position, "absolute" | "fixed") {
        let scope = positioner
            .and_then(|block| ancestors.iter().rposition(|&ancestor| ancestor == block))
            .map_or(1, |index| index + 1);
        reader.log(
            Query::Positioner(position == "fixed"),
            scope,
            hashed(positioner),
        );
        reader.log(
            Query::FlatParent,
            depth,
            hashed((
                parent,
                parent.and_then(|parent| styles.values[parent].get(Prop::DISPLAY)),
            )),
        );
    }
    let positioner = positioner.filter(|_| id != 0);
    // An inline box can be a part of the text of its block, so its out-of-flow boxes follow the containing block it
    // gives them even when it is their parent. When it has a box of its own, that is the same place.
    let parent_box = parent.filter(|&parent| {
        !matches!(position, "absolute" | "fixed")
            || doc.nodes[parent].tag.starts_with('#')
            || styles.values[parent].get(Prop::DISPLAY).unwrap_or("inline") != "inline"
    });
    let viewport = || viewport(reader, &units);
    let block_size = if position == "fixed" && positioner.is_none() {
        viewport()
    } else {
        positioner
            .filter(|&block| Some(block) != parent_box)
            .and_then(|block| {
                if block == 0 {
                    return viewport();
                }
                // Prefer the last layout to the estimate: missed min/max constraints can cause endless
                // redraws.
                let size = reader.block_size(view.host.borrow().page_state(view.id), block);
                reader.log(
                    Query::Block(block as u32),
                    0,
                    hashed(size.map(|size| size.map(f32::to_bits))),
                );
                size
            })
    };
    let containing = block_size.unwrap_or(containing);
    // Element building and layout recurse per level, so deep pages must not overflow the stack.
    if chain.len() > 256 || style.get(Prop::DISPLAY).is_some_and(|s| s == "none") {
        return renderer::node().hidden().into();
    }
    let opacity = parsed.opacity;
    // Use element opacity at zero. A hidden layer still costs two render passes.
    // A leaf fades each of its paints alone, which is right only when they don't overlap. Paints outside the border
    // box (outer shadows, an outline) can overlap only each other, and paints inside it only each other.
    let overlapping = || {
        let shadows = |inset| {
            style.get(Prop::BOX_SHADOW).map_or(0, |value| {
                (css::value::box_shadows(&units, value, inset).iter())
                    .filter(|shadow| !shadow.color.is_transparent())
                    .count()
            })
        };
        let images = style
            .get(Prop::BACKGROUND_IMAGE)
            .filter(|value| *value != "none")
            .map_or(0, |value| css::value::tokens::components(value, ',').len());
        let border = (units.borders(style, 0.).into_iter())
            .zip(parsed.border_colors)
            .any(|(width, color)| width > 0. && color.a > 0.);
        let outline = style.get(Prop::OUTLINE_STYLE).and_then(|kind| {
            let offset = style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
            match kind {
                "none" | "hidden" => None,
                // A focus ring moves 1px into a border.
                "auto" => Some(offset < 0. || offset < 1. && border),
                _ => Some(offset < 0.),
            }
        });
        let content = cull::replaced(&node.tag)
            || matches!(
                node.tag.as_str(),
                "img" | "input" | "textarea" | "select" | "progress" | "meter" | "terminal"
            )
            || styles.pseudos.get(&id).is_some_and(|pseudos| {
                pseudos.iter().any(|pseudo| {
                    pseudo
                        .get(Prop::CONTENT)
                        .is_some_and(|value| !matches!(value, "none" | "normal"))
                        && pseudo.get(Prop::DISPLAY) != Some("none")
                })
            });
        let inside = shadows(true)
            + images
            + [
                parsed.background.is_some(),
                border,
                content,
                outline == Some(true),
            ]
            .into_iter()
            .filter(|&paints| paints)
            .count();
        inside > 1 || shadows(false) + usize::from(outline == Some(false)) > 1
    };
    let effect_layer = (made.layer
        || opacity > 0. && opacity < 1. && (!node.children.is_empty() || overlapping())
        || composited)
        .then_some(opacity);
    let selection_colors = match styles.selections.get(&id) {
        Some(selection) => style::selection_colors(style, Some(selection)),
        None => parsed.selection,
    };
    let face = &parsed.face;
    let line_height = parsed.line_height;
    let mut carry = Carry::default();
    if node.tag == "br" {
        let empty = starts_line(build, id);
        reader.log(Query::Break, depth, hashed(empty));
        let painter = view.clone();
        let ink = [Prop::CARET_COLOR, Prop::COLOR]
            .into_iter()
            .find_map(|prop| css::value::color::color(style.get(prop)?))
            .map_or(Color::hex(0x000000ff), css::value::color::rgba);
        let view = view.clone();
        let above = above(reader, &units, &parsed);
        // Like Chromium, the box of a `<br>` is the content area of its font, not the line.
        let [ascent, descent, _] = text
            .line_metrics(text.face(face), px(units.font.em * pixel_ratio))
            .map(|value| f32::from(value) / pixel_ratio);
        let mut br = renderer::canvas(
            move |bounds, _| {
                let view = &*view;
                view.host
                    .borrow_mut()
                    .state(view.id)
                    .geometry
                    .mirror
                    .insert(
                        id,
                        [
                            f32::from(bounds.left()),
                            f32::from(bounds.top()) + above - ascent,
                            0.,
                            ascent + descent,
                        ],
                    );
            },
            move |bounds, _, window| {
                let caret = painter.host.try_borrow().is_ok_and(|host| {
                    host.page_state(painter.id)
                        .is_some_and(|dom| dom.interaction.selection_caret == Some(id))
                });
                if caret {
                    window.paint_volatile();
                    super::super::paint::text::paint_caret(
                        &painter,
                        [id, 0, 0],
                        Rect::new(bounds.origin, size(px(1.), px(line_height))),
                        ink,
                        window,
                    );
                }
            },
        )
        .h(px(above))
        .mb(px(line_height - above))
        .flex_shrink_0()
        .baseline(renderer::Baseline::Bottom);
        br = if empty { br.w_full() } else { br.w(px(0.)) };
        return br.into();
    }
    let flow = !matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid");
    // A box that skips its contents sizes as if empty, or as its `contain-intrinsic-size`. For a script that reads
    // the geometry of the contents, it builds them but keeps the size it had without them.
    let (skipped, forced, intrinsic, last) = if info[id].flags & cull::SKIPS != 0 {
        let last = view
            .host
            .borrow()
            .page_state(view.id)
            .and_then(|dom| dom.layout.cull.widths.get(&id).copied());
        let mut measured = measured.borrow_mut();
        let skips = cull::skips(info[id].flags, style, id, &measured, false);
        let forced = skips && measured.forced.contains(&id);
        // A box remembers the last size it had with its contents.
        let remembered = measured.remembered.entry(id).or_insert((None, true));
        let was = remembered.1;
        if skips && !was {
            remembered.0 = last.or(remembered.0);
        }
        remembered.1 = skips;
        let remembered = remembered.0;
        (
            skips && !forced,
            forced,
            skips.then(|| super::super::layout::grid::intrinsic_size(style, &units, remembered)),
            // The last size is the size without the contents only if the last build skipped them.
            last.filter(|_| was),
        )
    } else {
        (false, false, None, None)
    };
    let mut flat = Vec::new();
    let drawable = style::drawable_canvas(node, style);
    let mut pending: Vec<usize> = if skipped {
        Vec::new()
    } else if drawable {
        let boxes = |child: &&usize| !doc.nodes[**child].tag.starts_with('#');
        node.children.iter().rev().filter(boxes).copied().collect()
    } else if matches!(
        node.tag.as_str(),
        "svg" | "canvas" | "iframe" | "progress" | "meter" | "terminal"
    ) {
        Vec::new()
    } else {
        node.children.iter().rev().copied().collect()
    };
    // Spliced inline boxes, with the range of their content in `flat`. The end of a box is `!index` in `pending`.
    let mut cuts: Vec<(usize, usize, usize)> = Vec::new();
    // The parts of the spliced boxes: their place in `flat`, and the key of their text or their first and last block.
    let mut cut_parts: Vec<(usize, usize, super::super::paint::Piece)> = Vec::new();
    // The block `::before` and `::after` of spliced boxes: their place in `flat` and their index.
    let mut cut_pseudos: Vec<(usize, usize)> = Vec::new();
    while let Some(child) = pending.pop() {
        if child > usize::MAX / 2 {
            let cut = cuts[!child].0;
            if crate::render::inline::pseudo_blocks(styles, cut).is_some_and(|[_, after]| after) {
                cut_pseudos.push((flat.len(), 1));
                flat.push(cut);
            }
            cuts[!child].2 = flat.len();
        // A plain block is an element with a box, and its flags are closer than its node.
        } else if info[child].flags & cull::PLAIN_BLOCK != 0 {
            flat.push(child);
        } else if styles.values[child]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value == "contents")
        {
            pending.extend(doc.nodes[child].children.iter().rev());
        } else if flow
            && !drawable
            && crate::render::inline::holds_block_box(doc, styles, child)
            && {
                let spliced = crate::render::inline::spliced(doc, styles, child);
                reader.log(Query::Spliced(child as u32), depth, hashed(spliced));
                spliced
            }
        {
            pending.push(!cuts.len());
            cuts.push((child, flat.len(), flat.len()));
            pending.extend(doc.nodes[child].children.iter().rev());
            if crate::render::inline::pseudo_blocks(styles, child).is_some_and(|[before, _]| before)
            {
                cut_pseudos.push((flat.len(), 0));
                flat.push(child);
            }
        } else if !matches!(doc.nodes[child].tag.as_str(), "#comment" | "#doctype")
            && !(doc.nodes[child].tag == "br"
                && styles.values[child].get(Prop::DISPLAY) == Some("none"))
            && !(doc.nodes[child].tag == "#text"
                && (matches!(node.tag.as_str(), "select" | "optgroup")
                    // A closed <details> shows only its summary.
                    || node.tag == "details" && !node.attrs.contains_key("open")))
        {
            flat.push(child);
        }
    }
    if matches!(display, "flex" | "inline-flex" | "grid" | "inline-grid") {
        measured
            .borrow_mut()
            .note_runs(doc, styles, id, &flat, true);
    }
    let cut_ids: rustc_hash::FxHashSet<usize> = cuts.iter().map(|&(cut, ..)| cut).collect();
    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
    let line_justify = match (style.get(Prop::TEXT_ALIGN), rtl) {
        (Some("right" | "-webkit-right"), _)
        | (Some("end"), false)
        | (None | Some("start" | "justify"), true) => Some(renderer::AlignContent::End),
        (Some("center" | "-webkit-center"), _) => Some(renderer::AlignContent::Center),
        _ => None,
    };
    let line_align = Some(renderer::AlignItems::Baseline);
    let mut cx = Cx {
        reader,
        build,
        id,
        node,
        style,
        units,
        parsed: &parsed,
        display,
        position,
        containing,
        container,
        slot: &slot,
        pseudo,
        depth,
        multicol,
        flow,
        flat: &flat,
        cuts: &cuts,
        cut_ids: &cut_ids,
        cut_pseudos: &cut_pseudos,
        flexbox: matches!(display, "flex" | "inline-flex"),
        editable: text_field(node, style),
        stacking,
        broken,
        alt,
        sizeless,
        first_content: None,
        last_content: None,
        run_texts: RefCell::default(),
        rtl,
        line_justify,
        line_align,
        all_inline: false,
        hit: None,
    };
    cx.first_content = flat.iter().position(|&child| cx.content(child));
    cx.last_content = flat.iter().rposition(|&child| cx.content(child));
    let breaks = display != "inline"
        && flat.iter().any(|&child| doc.nodes[child].tag == "br")
        && cx.inline_text().is_none();
    // A multi-column box puts its lines into the columns, so its children never make the lines of the box itself.
    let all_inline = flow
        && !multicol
        && !flat.is_empty()
        && !breaks
        && flat.iter().all(|&child| {
            cx.inline_level(child) || styles.values[child].get(Prop::DISPLAY) == Some("none")
        });
    cx.all_inline = all_inline;
    // Use the in-flow image's ratio for auto height: Taffy drops a flex-row item's ratio with percentage
    // width.
    let (mut natural, mut natural_ratio) = match node.tag.as_str() {
        "img" | "video" => (None, None),
        _ => natural_size(doc, id, None, style)
            .map_or((None, None), |(size, ratio)| (Some(size), ratio)),
    };
    #[cfg(debug_assertions)]
    let checking = reader.checking;
    #[cfg(not(debug_assertions))]
    let checking = false;
    // Include every shared-style input in the key: plan, call, own reads and child facts.
    // A box whose children make no line, like spaces and hidden elements, has no baseline of its own.
    let share = (parsed.shareable
        && !checking
        && info[id].flags & cull::SKIPS == 0
        && slot.is_none()
        && parent_perspective.is_none()
        && natural.is_none()
        && id != 0
        && !broken
        && node.native_control(style).is_none()
        && !styles.pseudos.contains_key(&id)
        && !top_layer.contains(&id)
        && matches!(
            node.tag.as_str(),
            "div"
                | "span"
                | "a"
                | "p"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "nav"
                | "main"
                | "aside"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "b"
                | "i"
                | "strong"
                | "em"
                | "small"
                | "label"
                | "ul"
                | "ol"
                | "li"
                | "code"
                | "form"
        ))
    .then(|| {
        use std::hash::{Hash, Hasher};
        let texted = cx
            .inline_text()
            .map(|data| (data.text.is_empty(), data.greedy));
        let mut hasher = rustc_hash::FxHasher::default();
        (
            Rc::as_ptr(&parsed),
            containing.map(f32::to_bits),
            container.map(f32::to_bits),
            block_size.is_some(),
            flex_parent,
            columned,
            info[id].flags & !(cull::CHANGED | cull::CHILDREN_CHANGED | cull::GEOMETRY_CHANGED),
            (
                all_inline,
                node.children.is_empty(),
                texted,
                cx.blank(),
                cx.empty_inline(),
            ),
            // A box at the top of a shadow tree takes no floats from outside, so it gets no Flow::BLOCK.
            node.parent
                .is_none_or(|parent| doc.nodes[parent].tag.starts_with('#')),
        )
            .hash(&mut hasher);
        hasher.finish()
    });
    cx.hit = share.and_then(|key| {
        let shared = view.shared.borrow();
        let host = view.host.borrow();
        let dom = host.page_state(view.id);
        let here = |read: &Read| Read {
            query: read.query.moved(u32::MAX, id as u32).unwrap_or(read.query),
            ..*read
        };
        let found = shared.get(&key)?.iter().rev().find(|entry| {
            Rc::ptr_eq(&entry.plan, &parsed)
                && entry.reads.iter().all(|read| {
                    reader.replay(id, containing, container, None, dom, &here(read)) == Replay::Same
                })
        })?;
        reads
            .borrow_mut()
            .extend(found.reads.iter().map(|read| Read {
                scope: if read.scope == 0 {
                    0
                } else {
                    (depth + 2).saturating_sub(usize::from(read.scope)).max(1) as u16
                },
                reader: depth as u16,
                ..here(read)
            }));
        Some(found.style.clone())
    });
    let hit = &cx.hit;
    let mut el = match hit {
        Some(style) => renderer::shared(style.clone()).id(id),
        None => {
            let mut el = renderer::node()
                .id(id)
                .flex()
                .flex_col()
                .text_size(px(units.font.em))
                .font_family(face.family.clone())
                .font_weight(face.weight)
                .line_height(px(line_height));
            el.style().flex_shrink = if flex_parent { 1. } else { 0. };
            el.style().baseline = match display {
                "inline-block" if !flex_parent && slot.is_none() => renderer::Baseline::Last,
                "flex" | "inline-flex" | "grid" | "inline-grid" => renderer::Baseline::First,
                _ => renderer::Baseline::Auto,
            };
            el.style().block = matches!(
                display,
                "block" | "inline-block" | "list-item" | "flow-root"
            );
            el.style().contains = (u8::from(made.absolute) * renderer::Escape::ABSOLUTE)
                | (u8::from(made.fixed) * renderer::Escape::FIXED);
            el.style().escape = match position {
                "absolute" => renderer::Escape::ABSOLUTE,
                "fixed" => renderer::Escape::FIXED,
                _ => 0,
            };
            if position == "absolute" {
                el.style().placed = inset_axes(style);
            }
            el.style().scroll_anchor = (u8::from(style.get(Prop::OVERFLOW_ANCHOR) == Some("none"))
                * renderer::ScrollAnchor::NONE)
                | (u8::from(position == "sticky") * renderer::ScrollAnchor::STICKY);
            // Only boxes in columns break, so others skip these reads.
            if columned {
                use renderer::Break;
                for (props, forced, avoid) in [
                    (
                        [Prop::BREAK_BEFORE, Prop::PAGE_BREAK_BEFORE],
                        Break::BEFORE,
                        Break::AVOID_BEFORE,
                    ),
                    (
                        [Prop::BREAK_AFTER, Prop::PAGE_BREAK_AFTER],
                        Break::AFTER,
                        Break::AVOID_AFTER,
                    ),
                    (
                        [Prop::BREAK_INSIDE, Prop::PAGE_BREAK_INSIDE],
                        0,
                        Break::AVOID_INSIDE,
                    ),
                ] {
                    match props.iter().find_map(|&prop| style.get(prop)) {
                        Some("column") => el.style().breaks |= forced,
                        Some(value) if value.starts_with("avoid") => el.style().breaks |= avoid,
                        _ => {}
                    }
                }
                if matches!(
                    node.tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "video"
                        | "audio"
                        | "iframe"
                        | "embed"
                        | "object"
                        | "input"
                        | "textarea"
                        | "select"
                        | "button"
                        | "meter"
                        | "progress"
                        | "webview"
                ) || display.starts_with("inline-")
                {
                    // Like Chrome, atomic inline boxes are monolithic.
                    el.style().breaks |= Break::WHOLE;
                }
                let kept = [Prop::ORPHANS, Prop::WIDOWS].map(|prop| {
                    style
                        .get(prop)
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(2)
                });
                if kept != [2, 2] {
                    el.text_style().set_lines_kept(kept);
                }
            }
            {
                let text = el.text_style();
                text.set_letter_spacing(parsed.spacing[0]);
                text.set_word_spacing(parsed.spacing[1]);
                text.set_font_smoothing(!parsed.smoothing);
                let (indent, percent) = text_indent(style, &units);
                text.set_text_indent(px(indent));
                text.set_text_indent_percent(percent);
            }
            if face.style != renderer::FontStyle::Normal {
                el.text_style().set_font_style(face.style);
            }
            if face.synthesis != [true; 2] {
                el.text_style().set_font_synthesis(face.synthesis);
            }
            el
        }
    };
    if stacking {
        if hit.is_none() {
            el = el.stacking();
        }
        el = el.child(renderer::layers(true));
    }
    if all_inline || display == "inline" {
        if hit.is_none() {
            el = el.flex_row().flex_wrap();
            el.style().justify_content = line_justify;
            el.style().align_items = line_align;
            // A box that is stretched taller keeps its lines at the top, without space between them.
            el.style().align_content = Some(renderer::AlignContent::FlexStart);
            el.style().inline = renderer::Inline::Lines;
        }
        let merged = !text_field(node, style)
            && !matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
            && cx.inline_text().is_some();
        if !merged
            && flat.iter().any(|&child| {
                !doc.nodes[child].tag.starts_with('#')
                    && doc.nodes[child].tag != "br"
                    && styles.values[child].get(Prop::DISPLAY) != Some("inline")
                    && in_flow(doc, info, child)
                    && !crate::render::inline::makes_no_line(doc, styles, child)
            })
        {
            (carry.strut_due, carry.struts) = (display != "inline", true);
        }
    }
    // The content of replaced elements paints above the background layers, which are built later.
    let mut content_layers: Vec<Child> = Vec::new();
    let (mut grid_bar, mut picture) = (None, None);
    if broken || matches!(node.tag.as_str(), "canvas" | "terminal" | "img" | "video") {
        el = replaced(
            &cx,
            el,
            &mut natural,
            &mut natural_ratio,
            &mut content_layers,
            &mut grid_bar,
            &mut picture,
        );
    }
    if node.tag == "svg" {
        el = svg_box(&cx, el, &mut content_layers);
    }
    let stretched = |axis: usize| {
        let [size, start, end] = [
            [Prop::WIDTH, Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT],
            [Prop::HEIGHT, Prop::MARGIN_TOP, Prop::MARGIN_BOTTOM],
        ][axis];
        flex_parent
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_some_and(|value| value.starts_with("column"))
                == (axis == 0)
            && !matches!(position, "absolute" | "fixed")
            && style.get(size).is_none_or(|value| value == "auto")
            && [start, end]
                .iter()
                .all(|&prop| style.get(prop) != Some("auto"))
            && style
                .get(Prop::ALIGN_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    let input = (node.tag == "input").then(|| node.input_type());
    let appkit = node.native_control(style);
    if appkit.is_some() {
        super::super::paint::native::register(&view.controls, id);
    }
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = cx.editable;
    let dark = || {
        if style
            .get(Prop::COLOR_SCHEME)
            .is_some_and(|value| value.contains("dark") && value.contains("light"))
        {
            reader.log(Query::Theme, 0, hashed(crate::native::theme_state() & 1));
        }
        style::dark(style, crate::native::theme_state())
    };
    if input.is_some_and(|kind| kind.eq_ignore_ascii_case("file"))
        && let Some(part) = styles
            .scrollbars
            .get(&id)
            .and_then(|parts| parts[1].as_ref())
    {
        // The button takes its box from `::file-selector-button`, which the agent sheet always gives.
        let [padding, margin] = [css::properties::PADDING, css::properties::MARGIN]
            .map(|sides| units.edges(part, sides, 0.));
        let borders = units.borders(part, 0.);
        let color = |prop| part.get(prop).and_then(css::value::color::paint);
        let radius = part
            .get(Prop::BORDER_TOP_LEFT_RADIUS)
            .and_then(|value| units.resolve(value, 0.))
            .unwrap_or(0.);
        let mut button = renderer::node()
            .flex_shrink_0()
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]))
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]))
            .border_t(px(borders[0]))
            .border_r(px(borders[1]))
            .border_b(px(borders[2]))
            .border_l(px(borders[3]))
            .border_color(color(Prop::BORDER_TOP_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .bg(color(Prop::BACKGROUND_COLOR).unwrap_or(renderer::Color::TRANSPARENT))
            .child("Choose File");
        if let Some(ink) = color(Prop::COLOR) {
            button = button.text_color(ink);
        }
        if let Some(size) = part
            .get(Prop::FONT_SIZE)
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| *size != units.font.em)
        {
            let text = button.text_style();
            text.set_font_size(px(size));
            text.set_line_height(px(units.font.lh * size / units.font.em));
        }
        if let Some(weight) = part
            .get(Prop::FONT_WEIGHT)
            .filter(|weight| Some(*weight) != style.get(Prop::FONT_WEIGHT))
            .and_then(|weight| match weight {
                "bold" => Some(700.),
                "normal" => Some(400.),
                weight => weight.parse().ok(),
            })
        {
            button.text_style().set_font_weight(weight);
        }
        button.style().corner_radii = renderer::Corners::all(px(radius).into()).into();
        el = el
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(button)
            .child(renderer::node().whitespace_nowrap().child("No file chosen"));
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            el = el.w(px(file_size(style, part, &units, text, pixel_ratio)[0]));
        }
    }
    if appkit == Some(renderer::ControlKind::Button) {
        let [padding, height] =
            super::super::layout::size::native_size(renderer::ControlKind::Button);
        if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
            let label = node.button_label();
            let run = style::text_run(style, &units, label.len());
            let width = text.shape_line(label, px(units.font.em), &run).line.width;
            el = el.w(px(layout_ceil(f32::from(width) + padding, pixel_ratio)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|value| value == "auto") {
            el = el.h(px(height));
        }
    }
    if appkit.is_none()
        && input.is_some_and(|kind| {
            ["button", "submit", "reset"]
                .iter()
                .any(|other| kind.eq_ignore_ascii_case(other))
        })
    {
        el = el.items_center().justify_center().child(SharedString::from(
            node.attrs.get("value").cloned().unwrap_or_else(|| {
                if input.is_some_and(|kind| kind.eq_ignore_ascii_case("reset")) {
                    "Reset".into()
                } else if input.is_some_and(|kind| kind.eq_ignore_ascii_case("submit")) {
                    "Submit".into()
                } else {
                    String::new()
                }
            }),
        ));
    }
    let field_line_height = if editable {
        field_line(node, style, &units, text, pixel_ratio)
    } else {
        0.
    };
    if editable && node.tag != "textarea" {
        let above = f32::from(text.baseline(
            text.face(&parsed.face),
            px(units.font.em),
            px(field_line_height),
            pixel_ratio,
        ));
        el.style().justify_content = Some(renderer::AlignContent::Center);
        el = el.child(
            renderer::node()
                .w(px(0.))
                .h(px(above))
                .mb(px(field_line_height - above))
                .flex_shrink_0()
                .baseline(renderer::Baseline::Bottom),
        );
    }
    let mut field_layer = None;
    if editable {
        if style.get(Prop::FIELD_SIZING) == Some("content") {
            reader.log(Query::Value(id as u32), 0, hashed(&node.value));
        }
        let text = node.shown_text(
            node.current_value()
                .map_or_else(|| doc.text(id), str::to_owned),
        );
        let placeholder = node.attrs.get("placeholder").cloned().unwrap_or_default();
        let font_size = units.font.em;
        let text_color = style
            .get(Prop::COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(renderer::Color::BLACK);
        let caret_color = style
            .get(Prop::CARET_COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(text_color);
        let pseudo = styles
            .text_pseudos
            .get(&id)
            .and_then(|pseudos| pseudos[3].as_ref());
        let mut placeholder_color = pseudo
            .and_then(|pseudo| css::value::color::paint(pseudo.get(Prop::COLOR)?))
            .unwrap_or(Color::hex(0x757575ff));
        placeholder_color.a *= pseudo
            .and_then(|pseudo| pseudo.get(Prop::OPACITY)?.parse::<f32>().ok())
            .map_or(1., |opacity| opacity.clamp(0., 1.));
        let single_line = node.tag != "textarea";
        let padding = units.edges(style, css::properties::PADDING, containing[0]);
        let empty = text.is_empty();
        let face = match pseudo {
            Some(pseudo) if empty => style::font(pseudo),
            _ => parsed.face.clone(),
        };
        let mut shown = if empty { placeholder } else { text };
        if let Some(mode) = pseudo
            .filter(|_| empty)
            .unwrap_or(style)
            .get(Prop::TEXT_TRANSFORM)
            .filter(|mode| *mode != "none")
        {
            let len = shown.len();
            style::text_transform(&mut shown, 0..len, mode);
        }
        let shown: Rc<str> = shown.into();
        let run = renderer::TextRun {
            len: shown.len(),
            font: face,
            color: if empty { placeholder_color } else { text_color },
            background_color: None,
            underline: None,
            strikethrough: None,
            overline: None,
            letter_spacing: style::spacing(style, &units, Prop::LETTER_SPACING),
            word_spacing: px(0.),
            font_size: None,
            inset: Default::default(),
            nowrap: false,
            tab_size: 8.,
            stroke: None,
        };
        if !single_line && style.get(Prop::FIELD_SIZING) == Some("content") {
            // The full width of the content box, so the text wraps as in the field.
            let mut sizer = renderer::text(shown.clone(), Some(vec![run.clone()]))
                .w(relative(1.))
                .min_h(px(field_line_height))
                .flex_shrink_0();
            sizer.style().visibility = renderer::Visibility::Hidden;
            el = el.child(sizer);
        }
        let handle = view
            .host
            .borrow_mut()
            .state(view.id)
            .geometry
            .scroll
            .entry(id)
            .or_default()
            .clone();
        let field = super::super::paint::field::FieldText {
            selected: Default::default(),
            id,
            view: view.clone(),
            placeholder: empty,
            password: (node.password() && !empty)
                .then(|| node.current_value().unwrap_or("").to_owned()),
            text: shown,
            run,
            single_line,
            rtl: rtl && line_justify == Some(renderer::AlignContent::End),
            rows: Default::default(),
            caret: caret_color,
            selection: selection_colors,
            handle: handle.clone(),
        };
        let mut clip = renderer::node()
            .absolute()
            .track_scroll(&handle)
            .text_size(px(font_size))
            .line_height(px(field_line_height));
        clip.style().restrict_scroll_to_axis = true;
        clip.style().overscroll = overscroll(style);
        el = el.cursor(renderer::Cursor::Text);
        // Added after the background layers, which are absolute too and would paint over the text.
        field_layer = Some(if single_line {
            let line = renderer::node().flex_shrink_0().child(field);
            // A right-to-left row starts at the right, so text that overflows goes to the left and scrolls from there.
            let line = match (line_justify, rtl) {
                (Some(renderer::AlignContent::Center), _) => line.ml_auto().mr_auto(),
                (Some(renderer::AlignContent::End), false) => line.ml_auto(),
                (None, true) => line.mr_auto(),
                _ => line,
            };
            let mut clip = clip
                .top(px(padding[0]))
                .right(px(padding[1]))
                .bottom(px(padding[2]))
                .left(px(padding[3]))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .whitespace_nowrap();
            if rtl {
                clip.style().flex_direction = renderer::FlexDirection::RowReverse;
            }
            clip.child(line)
        } else {
            if rtl && line_justify == Some(renderer::AlignContent::End) {
                clip.style().flow |= renderer::Flow::RTL_BLOCK;
            }
            clip.style().overflow.x = renderer::Overflow::Scroll;
            clip.top_0()
                .right(px(0.))
                .bottom(px(0.))
                .left_0()
                .pt(px(padding[0]))
                .pb(px(padding[2]))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                // The text takes the side padding, so like the inner editor of Chrome, the end padding doesn't
                // follow text that overflows.
                .child(
                    renderer::Node::from(field)
                        .pl(px(padding[3]))
                        .pr(px(padding[1])),
                )
        });
    }
    if hit.is_none()
        && let Some(ink) = style
            .get(Prop::WEBKIT_TEXT_FILL_COLOR)
            .and_then(css::value::color::paint)
            .or(parsed.ink)
    {
        el = el.text_color(ink);
    }
    let inline_box = display == "inline"
        && !node.tag.starts_with('#')
        && !flex_parent
        && slot.is_none()
        && !matches!(
            node.tag.as_str(),
            "img" | "svg" | "canvas" | "input" | "button" | "select" | "textarea" | "iframe"
        )
        && !matches!(position, "absolute" | "fixed");
    let mut inline_fill = renderer::Color::TRANSPARENT;
    let mut clipped_fill = None;
    // With `background-clip: text`, the background paints in one layer that shows through the text.
    let text_clip = !matches!(node.tag.as_str(), "html" | "body")
        && style
            .get(Prop::BACKGROUND_CLIP)
            .is_some_and(|value| value.contains("text"));
    let mut text_fill = None;
    // The root element paints the background of the canvas, also when it comes from the body.
    let source = matches!(node.tag.as_str(), "html" | "body")
        .then(|| super::super::paint::background::canvas_source(doc, styles))
        .flatten();
    let propagated = node.tag == "body" && source == Some(id);
    let canvas = source.filter(|_| node.tag == "html" && node.parent == Some(0));
    let mut cell_background = parsed.background.filter(|_| !propagated);
    if cell_background.is_none() && style::role(styles, id) == style::CELL {
        cell_background = row_fill(doc, styles, id);
        let table = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
            .find(|&parent| !(style::ROW..=style::FOOTER).contains(&style::role(styles, parent)));
        reader.log(
            Query::RowFill,
            table.map_or(0, |table| reader.scope_of(id, table)),
            hashed(cell_background.map(|fill| fill.to_array().map(f32::to_bits))),
        );
    }
    if let Some(color) = cell_background {
        let blended = style
            .get(Prop::BACKGROUND_BLEND_MODE)
            .is_some_and(|value| value.split(',').any(|mode| mode.trim() != "normal"))
            && style
                .get(Prop::BACKGROUND_IMAGE)
                .is_some_and(|value| value != "none");
        // The color fills the clip box of the bottom layer.
        let clip = style.get(Prop::BACKGROUND_CLIP).unwrap_or("");
        let clip = if clip.contains(',') {
            let clips = css::value::tokens::components(clip, ',');
            let layers = style
                .get(Prop::BACKGROUND_IMAGE)
                .map_or(1, |value| css::value::tokens::components(value, ',').len());
            clips
                .get((layers.max(1) - 1) % clips.len().max(1))
                .map_or("", |clip| clip.trim())
        } else {
            clip.trim()
        };
        if blended {
        } else if text_clip {
            text_fill = Some(color);
        } else if inline_box {
            inline_fill = color;
        } else if matches!(clip, "padding-box" | "content-box") {
            clipped_fill = Some((color, clip == "content-box"));
        } else if hit.is_none() {
            el = el.bg(color);
        }
    }
    let overflow = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
        style.get(prop).unwrap_or(if node.tag == "textarea" {
            "auto"
        } else {
            "visible"
        })
    });
    let overflow = if node.tag == "body"
        && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
            .into_iter()
            .all(|prop| {
                reader
                    .parent(id, prop)
                    .is_none_or(|value| value == "visible")
            })
        && body_overflow_moves(doc, styles, id)
    {
        ["visible"; 2]
    } else {
        overflow
    };
    let page = matches!(node.tag.as_str(), "html" | "body");
    // `content-visibility` contains the paint of the contents, like `overflow: clip`.
    let overflow = if info[id].flags & cull::SKIPS != 0
        && !page
        && style
            .get(Prop::CONTENT_VISIBILITY)
            .is_some_and(|v| v != "visible")
    {
        overflow.map(|value| if value == "visible" { "clip" } else { value })
    } else {
        overflow
    };
    let boxed = !page
        && overflow
            .iter()
            .any(|value| !matches!(*value, "visible" | "clip"));
    let overflow = overflow.map(|value| match value {
        "auto" | "scroll" => renderer::Overflow::Scroll,
        "hidden" => renderer::Overflow::Hidden,
        "clip" if boxed => renderer::Overflow::Hidden,
        "clip" => renderer::Overflow::Clip,
        _ if boxed => renderer::Overflow::Scroll,
        _ => renderer::Overflow::Visible,
    });
    if hit.is_none() && overflow != [renderer::Overflow::Visible; 2] && !editable {
        el.style().overflow.x = overflow[0];
        el.style().overflow.y = overflow[1];
    }
    let scrolls = overflow.map(|axis| axis == renderer::Overflow::Scroll);
    let look = if scrolls.contains(&true) {
        let backdrop = backdrop(doc, styles, id);
        reader.log(
            Query::Backdrop(id as u32),
            backdrop.map_or(1, |(provider, _)| reader.scope_of(id, provider)),
            hashed(backdrop),
        );
        reader.log(Query::Theme, 0, hashed(crate::native::theme_state() & 1));
        scrollbar::Look::new(
            style,
            styles.scrollbars.get(&id).map(|parts| &**parts),
            &units,
            backdrop.map(|(_, ink)| ink),
        )
        .map(|mut look| {
            if page {
                look.forced = [false; 2];
            }
            look
        })
    } else {
        None
    };
    let reserved = look.as_ref().map_or([0.; 2], |look| {
        // Make the handle before the read, or the read logs none and its replay finds the handle.
        let mut host = view.host.borrow_mut();
        let dom = host.state(view.id);
        let handle = dom.geometry.scroll.entry(id).or_default();
        reader.log(Query::Scroll(id as u32), 0, hashed(scrolled(Some(handle))));
        look.space(look.shown(scrolls, handle.extent()))
    });
    let mut padding = parsed.padding.unwrap_or_else(|| {
        units
            .edges(style, css::properties::PADDING, containing[0])
            .map(|value| value.max(0.))
    });
    padding[1] += reserved[1];
    padding[2] += reserved[0];
    let opposite = if style
        .get(Prop::SCROLLBAR_GUTTER)
        .is_some_and(|value| value.contains("both-edges"))
    {
        reserved[1]
    } else {
        0.
    };
    padding[3] += opposite;
    if hit.is_none() {
        el = el
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]));
    }
    let mut margin = margins(
        build,
        id,
        &units,
        containing[0],
        flex_parent,
        slot.is_some(),
    );
    if slot.is_none() {
        reader.log(Query::Margins, depth, hashed(margin.map(f32::to_bits)));
    }
    if hit.is_none() {
        el = el
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]));
    }
    if node.tag == "legend"
        && let Some(fieldset) = node.parent
        && doc.nodes[fieldset].tag == "fieldset"
        && let Some((legend, height, _)) = legend(doc, styles, fieldset)
        && legend == id
    {
        let left = styles.lengths[fieldset].edges(
            &styles.values[fieldset],
            css::properties::PADDING,
            containing[0],
        )[3];
        el = el.absolute().top(px(-height)).left(px(left));
    }
    for (side, prop) in css::properties::MARGIN.iter().enumerate() {
        if hit.is_none() && style.get(*prop).is_some_and(|s| s == "auto") {
            el = match side {
                0 => el.mt_auto(),
                1 => el.mr_auto(),
                2 => el.mb_auto(),
                _ => el.ml_auto(),
            };
        }
    }
    let rtl_parent = slot.is_none() && reader.parent(id, Prop::DIRECTION) == Some("rtl");
    if rtl_parent && hit.is_none() {
        el.style().flow |= renderer::Flow::RTL;
    }
    if hit.is_none() && style.get(Prop::DIRECTION) == Some("rtl") {
        el.style().flow |= renderer::Flow::RTL_BLOCK;
    }
    if hit.is_none()
        && !flex_parent
        && rtl_parent
        && style.get(Prop::DISPLAY).is_some_and(|value| {
            matches!(
                value,
                "block" | "list-item" | "table" | "flex" | "grid" | "flow-root"
            )
        })
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && style
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && style.get(Prop::WIDTH).is_some_and(|value| value != "auto")
        && [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
            .into_iter()
            .all(|prop| style.get(prop).is_none_or(|value| value != "auto"))
    {
        el = el.ml_auto();
    }
    let ink = parsed.ink.unwrap_or(Color::BLACK);
    let mut borders = parsed
        .borders
        .unwrap_or_else(|| units.borders(style, containing[0]));
    let sides = css::properties::BORDERS.map(|[width, kind, color]| (width, kind, color));
    let mut border_colors = parsed.border_colors;
    let mut border_kinds = sides.map(|(_, kind, _)| style.get(kind).unwrap_or("none"));
    if inline_box {
        let [above, below] = [padding[0] + borders[0], padding[2] + borders[2]];
        // Like browsers, padding and borders of an inline box hang outside its line, hence the negative
        // margins.
        margin[0] = -above;
        margin[2] = -below;
        if hit.is_none() {
            el = el.mt(px(margin[0])).mb(px(margin[2]));
            // Like Chrome, an empty inline box with inline margins, padding or borders makes a line, so it
            // takes the line height and its baseline.
            if !styles.pseudos.contains_key(&id)
                && cx.empty_inline()
                && !crate::render::inline::makes_no_line(doc, styles, id)
            {
                let [ascent, descent, _] = text
                    .line_metrics(text.face(&parsed.face), px(units.font.em * pixel_ratio))
                    .map(f32::from);
                let lead = ((units.font.lh * pixel_ratio - ascent - descent) / 2.).floor();
                let baseline = (lead + ascent) / pixel_ratio;
                let mut mark = renderer::node()
                    .h(px(baseline))
                    .mb(px(-baseline))
                    .baseline(renderer::Baseline::Bottom);
                mark.style().baseline_source = true;
                el = el.min_h(px(units.font.lh + above + below)).child(mark);
            }
        }
        let same = border_colors.iter().all(|color| *color == border_colors[0]);
        // A border image replaces the normal border.
        let imaged = style
            .get(Prop::BORDER_IMAGE_SOURCE)
            .is_some_and(|value| value != "none");
        let border = if same && !imaged && borders.iter().any(|width| *width > 0.) {
            border_colors[0]
        } else {
            renderer::Color::TRANSPARENT
        };
        if border.a > 0. || inline_fill.a > 0. {
            let font = text.face(&parsed.face);
            let size_px = px(units.font.em);
            let metrics = text.metrics(font);
            let height = metrics.ascent * size_px.0 - metrics.descent * size_px.0;
            let line_height = units.font.lh;
            let radii = super::super::effects::clip::radii(style, &units, || containing);
            let (sides, pad_y, fill) = (borders, padding[0] + padding[2], inline_fill);
            let dashed = matches!(border_kinds[0], "dashed");
            el = el.child(
                renderer::canvas(
                    |_, _| (),
                    move |bounds, _, window| {
                        let step = px(line_height.max(1.));
                        let mask = window.content_mask().bounds;
                        let quad = size(
                            bounds.size.width + px(sides[1] + sides[3]),
                            px(height + above + below),
                        );
                        let start = bounds.top() + px((line_height - height) / 2. - sides[0]);
                        let lines =
                            (((bounds.size.height - px(pad_y)) / step).round() as usize).max(1);
                        for line in 0..lines {
                            let top = start + step * line as f32;
                            if top > mask.bottom() {
                                window.cull();
                                break;
                            }
                            if top + quad.height < mask.top() {
                                window.cull();
                                continue;
                            }
                            window.draw_box(
                                Rect::new(point(bounds.left() - px(sides[3]), top), quad),
                                radii.resolve(quad),
                                fill,
                                renderer::Border {
                                    widths: renderer::Edges {
                                        top: px(sides[0]),
                                        right: px(sides[1]),
                                        bottom: px(sides[2]),
                                        left: px(sides[3]),
                                    },
                                    color: border,
                                    style: if dashed {
                                        renderer::BorderStyle::Dashed
                                    } else {
                                        renderer::BorderStyle::Solid
                                    },
                                    sides: 0,
                                },
                            );
                        }
                    },
                )
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .size_full(),
            );
            if same {
                border_colors = [renderer::Color::TRANSPARENT; 4];
            }
        }
    }
    let list = (node.tag == "select")
        .then(|| {
            match node
                .attrs
                .get("size")
                .and_then(|size| size.trim().parse::<usize>().ok())
            {
                Some(rows) if rows > 1 => Some(rows),
                _ => node.attrs.contains_key("multiple").then_some(4),
            }
        })
        .flatten();
    let native = super::super::paint::control::kind(&node.tag, input, list.is_some(), style).map(
        |(control, boxed, plain)| {
            let number = |name: &str| {
                node.attrs
                    .get(name)
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|value| value.is_finite())
            };
            let [min, max] = match control {
                super::super::paint::control::Control::Range => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(100.)]
                }
                super::super::paint::control::Control::Meter => {
                    [number("min").unwrap_or(0.), number("max").unwrap_or(1.)]
                }
                _ => [0., number("max").filter(|&max| max > 0.).unwrap_or(1.)],
            };
            let max = max.max(min);
            let value = match control {
                super::super::paint::control::Control::Range => {
                    let value = node
                        .current_value()
                        .and_then(|value| value.trim().parse::<f32>().ok())
                        .map_or((min + max) / 2., |value| value.clamp(min, max));
                    // Like the value property, the thumb snaps to a step from `min`.
                    match node.attrs.get("step").map(|step| step.trim()) {
                        Some(step) if step.eq_ignore_ascii_case("any") => value,
                        step => {
                            let step = step
                                .and_then(|step| step.parse::<f32>().ok())
                                .filter(|step| *step > 0.)
                                .unwrap_or(1.);
                            let steps = ((value - min) / step).round();
                            min + if min + steps * step > max {
                                steps - 1.
                            } else {
                                steps
                            } * step
                        }
                    }
                }
                _ => number("value").unwrap_or(
                    if control == super::super::paint::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    },
                ),
            };
            let mut dark = dark();
            let accent = match control {
                super::super::paint::control::Control::Meter => {
                    let low = number("low").unwrap_or(min).clamp(min, max);
                    let high = number("high").unwrap_or(max).clamp(low, max);
                    let region = |value: f32| u8::from(value >= low) + u8::from(value > high);
                    let optimum = region(
                        number("optimum")
                            .unwrap_or((min + max) / 2.)
                            .clamp(min, max),
                    );
                    let current = region(value.clamp(min, max));
                    let colors = if dark {
                        [0x74b374, 0xf2c812, 0xe98f6d]
                    } else {
                        [0x107c10, 0xffb900, 0xd83b01]
                    };
                    if optimum == current {
                        colors[0]
                    } else if optimum == 1 || current == 1 {
                        colors[1]
                    } else {
                        colors[2]
                    }
                }
                super::super::paint::control::Control::Color => node
                    .current_value()
                    .and_then(css::value::color::color)
                    .map_or(0, |(color, _)| color),
                _ if node.attrs.contains_key("disabled") => 0x767676,
                _ => match style
                    .get(Prop::ACCENT_COLOR)
                    .and_then(css::value::color::color)
                {
                    Some((color, _)) => {
                        dark = super::super::paint::control::accent_dark(
                            control,
                            node.checked(),
                            color,
                            dark,
                        );
                        color
                    }
                    None if dark => 0x99c8ff,
                    None => 0x0075ff,
                },
            };
            super::super::paint::control::Look {
                control,
                boxed,
                plain,
                checked: node.checked(),
                disabled: node.attrs.contains_key("disabled"),
                hover: node.state & crate::css::selector::HOVER != 0,
                active: node.state & crate::css::selector::ACTIVE != 0,
                dark,
                accent,
                ink: style
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .map_or(0, |(color, _)| color),
                value: if value < min
                    || max <= min && control != super::super::paint::control::Control::Progress
                {
                    if control == super::super::paint::control::Control::Progress {
                        -1.
                    } else {
                        0.
                    }
                } else {
                    ((value - min) / (max - min).max(f32::EPSILON)).clamp(0., 1.)
                },
            }
        },
    );
    let native = native.filter(|_| appkit.is_none());
    if native.is_some() {
        reader.log(Query::Look(id as u32), 0, hashed(node.state & LOOK));
    }
    #[cfg(debug_assertions)]
    if let Some(look) = &native {
        reader.extra.set(hashed((
            reader.extra.get(),
            [
                look.hover,
                look.active,
                look.checked,
                look.disabled,
                look.dark,
            ],
            [look.accent, look.ink, look.value.to_bits()],
        )));
    }
    if let Some(look) = &native {
        if look.boxed {
            let (edge, fill) = super::super::paint::control::box_colors(look);
            for side in 0..4 {
                padding[side] += (borders[side] - 1.).max(0.);
                borders[side] = borders[side].min(1.);
                border_kinds[side] = "solid";
                if let Some(edge) = edge {
                    border_colors[side] = Color::hex(edge << 8 | 0xff);
                }
            }
            if let Some(fill) = fill {
                el = el.bg(Color::hex(fill << 8 | 0xff));
            }
        }
        if let Some(rows) = list
            && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
        {
            // Options have `line-height: normal` and `min-height: 1.2em`.
            let normal = text
                .line_metrics(text.face(&parsed.face), px(units.font.em * pixel_ratio))
                .map(f32::from)
                .iter()
                .sum::<f32>()
                / pixel_ratio;
            let row = (normal.max(1.2 * units.font.em) * 64.).round() / 64. + 1.;
            el = el.h(px(rows as f32 * row
                + padding[0]
                + padding[2]
                + borders[0]
                + borders[2]));
        }
        if look.control == super::super::paint::control::Control::Menu {
            padding[1] += 16.;
            padding[3] += 4.;
            if style.get(Prop::WIDTH).is_none_or(|value| value == "auto") {
                let (font, size) = (parsed.face.clone(), px(units.font.em));
                let mut widest = 0f32;
                let mut stack = node.children.clone();
                while let Some(child) = stack.pop() {
                    match doc.nodes[child].tag.as_str() {
                        "optgroup" => stack.extend(&doc.nodes[child].children),
                        "option" => {
                            let label = doc
                                .text(child)
                                .split_ascii_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            let run = renderer::TextRun {
                                len: label.len(),
                                font: font.clone(),
                                color: renderer::Color::BLACK,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                overline: None,
                                letter_spacing: px(0.),
                                word_spacing: px(0.),
                                font_size: None,
                                inset: Default::default(),
                                nowrap: false,
                                tab_size: 8.,
                                stroke: None,
                            };
                            widest = widest
                                .max(f32::from(text.shape_line(&label, size, &run).line.width));
                        }
                        _ => {}
                    }
                }
                el = el.min_w(px((widest * pixel_ratio).ceil() / pixel_ratio
                    + padding[1]
                    + padding[3]
                    + borders[1]
                    + borders[3]));
            }
            if !node.attrs.contains_key("multiple")
                && node
                    .attrs
                    .get("size")
                    .is_none_or(|size| size.trim().parse::<u32>().is_ok_and(|size| size <= 1))
                && style.get(Prop::HEIGHT).is_none_or(|value| value == "auto")
            {
                let face = text.face(&parsed.face);
                let [ascent, descent, _] = text
                    .line_metrics(face, px(units.font.em * pixel_ratio))
                    .map(f32::from);
                let height = px((ascent + descent) / pixel_ratio
                    + 2.
                    + padding[0]
                    + padding[2]
                    + borders[0]
                    + borders[2]);
                el = if stretched(1) {
                    el.min_h(height)
                } else {
                    el.h(height)
                }
                .justify_center();
            }
        }
        el = el
            .pt(px(padding[0]))
            .pr(px(padding[1]))
            .pb(px(padding[2]))
            .pl(px(padding[3]));
    }
    let legend = (node.tag == "fieldset")
        .then(|| legend(doc, styles, id))
        .flatten()
        .filter(|&(_, height, _)| height > borders[0]);
    if let Some((legend, height, middle)) = legend {
        el = el.child(fieldset_border(
            legend,
            height,
            middle,
            borders,
            border_colors,
            border_kinds,
            view.clone(),
        ));
        borders[0] = height;
        border_colors = [renderer::Color::TRANSPARENT; 4];
        border_kinds = ["solid"; 4];
    }
    let border_image = hit
        .is_none()
        .then(|| super::super::paint::background::border_image(style, units, id, 0, borders, view))
        .flatten();
    if border_image.is_some() {
        border_colors = [renderer::Color::TRANSPARENT; 4];
    }
    if hit.is_none() {
        el = el
            .border_t(px(borders[0]))
            .border_r(px(borders[1]))
            .border_b(px(borders[2]))
            .border_l(px(borders[3]));
    }
    let visible = (0..4).filter(|&side| borders[side] > 0.);
    let first = visible.clone().next();
    // Borders draw as their own child when the box can't draw them: shaded or mixed styles, or image layers
    // that reach under the border and must paint below it.
    let shaded = visible.clone().any(|side| {
        matches!(border_kinds[side], "groove" | "ridge" | "inset" | "outset")
            || first.is_some_and(|first| border_kinds[side] != border_kinds[first])
    }) || first.is_some()
        && style
            .get(Prop::BACKGROUND_IMAGE)
            .is_some_and(|value| value != "none")
        && css::value::tokens::components(
            style
                .get(Prop::BACKGROUND_CLIP)
                .filter(|clip| !clip.is_empty())
                .unwrap_or("border-box"),
            ',',
        )
        .iter()
        .any(|clip| !matches!(clip.trim(), "padding-box" | "content-box"));
    if hit.is_none()
        && let Some(first) = first
    {
        if !shaded
            && visible
                .clone()
                .all(|side| border_colors[side] == border_colors[first])
        {
            el = el.border_color(border_colors[first]);
            el.style().border_style = super::super::effects::border_style(border_kinds[first]);
        } else {
            el = el.border_color(renderer::Color::TRANSPARENT);
        }
    }
    let extra = if style
        .get(Prop::BOX_SIZING)
        .is_some_and(|value| value == "border-box")
    {
        [0.; 2]
    } else {
        [
            padding[1] + padding[3] + borders[1] + borders[3] - reserved[1] - opposite,
            padding[0] + padding[2] + borders[0] + borders[2] - reserved[0],
        ]
    };
    let floor = |axis: usize, size: f32| {
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
    };
    if editable {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let auto =
            [Prop::WIDTH, Prop::HEIGHT].map(|prop| style.get(prop).is_none_or(|v| v == "auto"));
        let percent = style.get(Prop::WIDTH).is_some_and(|v| v.ends_with('%'));
        let [width, height] = field_size(
            doc,
            id,
            style,
            &units,
            text,
            pixel_ratio,
            auto[0] || percent,
            None,
        );
        if auto[0] {
            if !stretched(0)
                && !(matches!(position, "absolute" | "fixed")
                    && [Prop::LEFT, Prop::RIGHT]
                        .iter()
                        .all(|&prop| style.get(prop).is_some_and(|value| value != "auto")))
            {
                el = el.w(px(width + chrome[0]));
            }
            if let Some(min) = floor(0, width + chrome[0]).filter(|_| node.tag != "textarea") {
                el = el.min_w(px(min));
            }
        } else if percent {
            el = el.child(super::super::paint::field::natural_width(px(width)));
        }
        if auto[1] && (node.tag != "textarea" || style.get(Prop::FIELD_SIZING) != Some("content")) {
            el = el.min_h(px(height + chrome[1]));
        }
    }
    let flexbox = cx.flexbox;
    let basis = || {
        [Prop::WIDTH, Prop::HEIGHT].map(|prop| {
            let base = containing[usize::from(prop == Prop::HEIGHT)];
            style
                .get(prop)
                .and_then(|length| units.resolve(length, base))
                .unwrap_or(base)
        })
    };
    let radius = parsed
        .radii
        .unwrap_or_else(|| super::super::effects::clip::radii(style, &units, basis));
    if hit.is_none() {
        el.style().corner_radii = radius;
    }
    if native.as_ref().is_some_and(|look| look.boxed) {
        el.style().corner_radii = renderer::Corners::all(px(2.).into()).into();
    }
    let mut inner_shadows = Vec::new();
    let mut intrinsic_width = None;
    let column_stretched = || {
        slot.is_some()
            && style
                .get(Prop::JUSTIFY_SELF)
                .filter(|value| *value != "auto")
                .or_else(|| reader.parent(id, Prop::JUSTIFY_ITEMS))
                .is_none_or(|value| matches!(value, "normal" | "stretch"))
            || flex_parent
                && reader
                    .parent(id, Prop::FLEX_DIRECTION)
                    .is_some_and(|value| value.starts_with("column"))
                && style
                    .get(Prop::ALIGN_SELF)
                    .filter(|value| *value != "auto")
                    .or_else(|| reader.parent(id, Prop::ALIGN_ITEMS))
                    .is_none_or(|value| matches!(value, "normal" | "stretch"))
    };
    if hit.is_some()
        && let Some(value) = style.get(Prop::BOX_SHADOW)
    {
        inner_shadows = css::value::box_shadows(&units, value, true);
    }
    // The left and right insets of a positioned box take room from its width.
    let inset_x = [Prop::LEFT, Prop::RIGHT].map(|prop| {
        style
            .get(prop)
            .filter(|_| matches!(position, "absolute" | "fixed"))
            .and_then(|v| units.resolve(v, containing[0]))
    });
    // Like Chrome, a box in a line takes percent heights from the height of its block: `Some(true)` when it is a
    // fixed length, and `Some(false)` when it is `auto`, which makes them `auto` too.
    let line_base = (display.starts_with("inline-")
        && [Prop::HEIGHT, Prop::MIN_HEIGHT, Prop::MAX_HEIGHT]
            .iter()
            .any(|&prop| style.get(prop).is_some_and(|value| value.contains('%')))
        && !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed"))
    .then(|| {
        let definite = definite_line(doc, styles, id);
        reader.log(Query::LineBase(id as u32), 0, hashed(definite));
        definite
    });
    // The content of a details box is in a slot of auto height, so its percent heights and offsets are `auto`.
    let in_details = !flex_parent
        && slot.is_none()
        && node.tag != "summary"
        && !matches!(position, "absolute" | "fixed")
        && parent.is_some_and(|parent| doc.nodes[parent].tag == "details");
    // The initial `start` follows the direction of the box.
    if hit.is_none()
        && style.get(Prop::TEXT_ALIGN).is_none()
        && rtl != (reader.parent(id, Prop::DIRECTION) == Some("rtl"))
    {
        el = if rtl { el.text_right() } else { el.text_left() };
    }
    for (prop, value) in style.iter().filter(|_| hit.is_none()) {
        let value: &str = value;
        let vertical = matches!(
            prop,
            Prop::HEIGHT | Prop::MIN_HEIGHT | Prop::MAX_HEIGHT | Prop::TOP | Prop::BOTTOM
        );
        if in_details && vertical && value.contains('%') {
            continue;
        }
        let base = containing[usize::from(vertical)];
        let length = || units.resolve(value, base);
        // The root element takes percent heights from the viewport, which its parent box does not fix. Like
        // Chrome, an inline-level box takes them from the fixed height of its block, which its line doesn't have.
        // A grid item takes them from its grid area.
        let page_size = || match vertical && parent == Some(0) {
            true => viewport(),
            false => (vertical && line_base == Some(true)
                || slot.is_some() && !style::table_part(doc, styles, id))
            .then_some(containing),
        };
        match prop {
            Prop::WIDTH | Prop::HEIGHT if sizeless => {}
            Prop::WIDTH
            | Prop::HEIGHT
            | Prop::MIN_WIDTH
            | Prop::MAX_WIDTH
            | Prop::MIN_HEIGHT
            | Prop::MAX_HEIGHT => {
                if vertical && line_base == Some(false) && value.contains('%') {
                    continue;
                }
                let axis = usize::from(vertical);
                let length: renderer::Length = if let Some(percent) = percentage(value) {
                    match block_size.or_else(page_size) {
                        Some(_) => px(percent.max(0.) / 100. * base + extra[axis]).into(),
                        None => renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis])),
                    }
                } else if let Some((fraction, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    match page_size() {
                        Some(_) => px(fraction * base + offset + extra[axis]).into(),
                        None => renderer::Length::Calc(fraction, px(offset + extra[axis])),
                    }
                } else if let Some(length) = (block_size.is_none() && page_size().is_none())
                    .then(|| percent_bound(&units, value, extra[axis], el.style()))
                    .flatten()
                {
                    length
                } else if let Some(length) = length() {
                    px(length.max(0.) + extra[axis]).into()
                } else if matches!(prop, Prop::WIDTH | Prop::HEIGHT)
                    && value == "auto"
                    && node.tag != "svg"
                    && !(editable && prop == Prop::WIDTH)
                    || prop == Prop::WIDTH
                        && style::table_box(style::role(styles, id))
                        && value == "fit-content"
                        && column_stretched()
                    || prop == Prop::WIDTH
                        && value == "fit-content"
                        && matches!(position, "absolute" | "fixed")
                        && inset_x.contains(&None)
                {
                    renderer::Length::Auto
                } else if prop == Prop::WIDTH
                    && style::table_box(style::role(styles, id))
                    && value == "fit-content"
                {
                    // The grid's measure child gives the content sizes, and these bounds clamp a stretched width.
                    let [min, max, ..] = super::super::layout::grid::intrinsic::intrinsic(
                        doc,
                        id,
                        styles,
                        None,
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let [min, max] = [min, max].map(|size| {
                        layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio)
                    });
                    intrinsic_width = Some(max.min(min.max(containing[0] - margin[1] - margin[3])));
                    if style.get(Prop::MIN_WIDTH).is_none() && style::role(styles, id) != 3 {
                        el = el.min_w(px(min));
                    }
                    if style.get(Prop::MAX_WIDTH).is_none() {
                        el = el.max_w(px(max));
                    }
                    renderer::Length::Auto
                } else if matches!(value, "stretch" | "-webkit-fill-available") {
                    relative(1.)
                } else if matches!(value, "fit-content" | "max-content" | "min-content")
                    && axis == 0
                {
                    let [min, max, ..] = super::super::layout::grid::intrinsic::intrinsic(
                        doc,
                        id,
                        styles,
                        Some(containing[0]),
                        None,
                        text,
                        &mut measured.borrow_mut(),
                    );
                    let size = match value {
                        "min-content" => min,
                        "max-content" => max,
                        _ => {
                            max.min(min.max(containing[0] - inset_x.iter().flatten().sum::<f32>()))
                        }
                    };
                    let size = layout_ceil((size - margin[1] - margin[3]).max(0.), pixel_ratio);
                    intrinsic_width = Some(size);
                    if value == "fit-content" && style.get(Prop::MAX_WIDTH).is_none() {
                        el = el.max_w(renderer::Length::Calc(1., px(-margin[1] - margin[3])));
                    }
                    px(size).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::WIDTH => el.w(length),
                    Prop::HEIGHT => el.h(length),
                    Prop::MIN_WIDTH => el.min_w(length),
                    Prop::MAX_WIDTH => el.max_w(length),
                    // A field's rows give its auto height, which an author min-height only raises.
                    Prop::MIN_HEIGHT => match (el.style().min_size.height, length) {
                        (renderer::Length::Px(rows), renderer::Length::Px(min)) if editable => {
                            el.min_h(rows.max(min))
                        }
                        _ => el.min_h(length),
                    },
                    _ => el.max_h(length),
                };
            }
            Prop::ROW_GAP | Prop::COLUMN_GAP if flexbox => {
                let Some(length) = length() else { continue };
                el = if prop == Prop::ROW_GAP {
                    el.gap_y(px(length.max(0.)))
                } else {
                    el.gap_x(px(length.max(0.)))
                };
            }
            Prop::BOX_SHADOW => {
                el.style().box_shadow = css::value::box_shadows(&units, value, false);
                inner_shadows = css::value::box_shadows(&units, value, true);
            }
            Prop::ASPECT_RATIO => {
                if !(value.contains("auto") && natural_ratio.is_some())
                    && let Some(ratio) = aspect_ratio(value)
                {
                    el.style().aspect_ratio = Some(ratio);
                    el = only_auto_ratio(el, style);
                }
            }
            Prop::DISPLAY if matches!(value, "grid" | "inline-grid") => el = el.grid(),
            Prop::DISPLAY if matches!(value, "flex" | "inline-flex") => {
                el = el.flex().flex_row();
                if rtl {
                    el.style().flex_direction = renderer::FlexDirection::RowReverse;
                }
            }
            Prop::FLEX_DIRECTION if flexbox => {
                if rtl && value.starts_with("column") {
                    el.style().flow |= renderer::Flow::RTL_ITEMS;
                }
                el.style().flex_direction = match (value, rtl) {
                    ("row-reverse", false) | ("row", true) => renderer::FlexDirection::RowReverse,
                    ("column", _) => renderer::FlexDirection::Column,
                    ("column-reverse", _) => renderer::FlexDirection::ColumnReverse,
                    _ => renderer::FlexDirection::Row,
                }
            }
            Prop::FLEX_WRAP if flexbox => {
                el.style().flex_wrap = match value {
                    "wrap" => renderer::FlexWrap::Wrap,
                    "wrap-reverse" => renderer::FlexWrap::WrapReverse,
                    _ => renderer::FlexWrap::NoWrap,
                }
            }
            Prop::FLEX_GROW | Prop::FLEX_SHRINK if flex_parent => {
                if let Some(factor) = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    if prop == Prop::FLEX_GROW {
                        el.style().flex_grow = factor;
                    } else {
                        el.style().flex_shrink = factor;
                    }
                }
            }
            Prop::FLEX_BASIS if flex_parent => {
                let axis = usize::from(
                    reader
                        .parent(id, Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column")),
                );
                let base = containing[axis];
                el.style().flex_basis = if let Some(percent) = percentage(value) {
                    renderer::Length::Calc(percent.max(0.) / 100., px(extra[axis]))
                } else if let Some(length) = units.resolve(value, base) {
                    px(length.max(0.) + extra[axis]).into()
                } else {
                    renderer::Length::Auto
                };
            }
            Prop::ALIGN_ITEMS | Prop::ALIGN_SELF if flex_parent || flexbox => {
                let align = match value {
                    "center" => renderer::AlignItems::Center,
                    "flex-start" | "start" | "self-start" => renderer::AlignItems::FlexStart,
                    "flex-end" | "end" | "self-end" => renderer::AlignItems::FlexEnd,
                    "baseline" | "first baseline" => renderer::AlignItems::Baseline,
                    "last baseline" => renderer::AlignItems::LastBaseline,
                    "stretch" | "normal" => renderer::AlignItems::Stretch,
                    _ => continue,
                };
                if prop == Prop::ALIGN_SELF {
                    el.style().align_self = Some(align);
                } else if flexbox {
                    el.style().align_items = Some(align);
                }
            }
            Prop::ALIGN_CONTENT | Prop::JUSTIFY_CONTENT if flexbox => {
                let inline = prop == Prop::JUSTIFY_CONTENT
                    && !style
                        .get(Prop::FLEX_DIRECTION)
                        .is_some_and(|v| v.starts_with("column"));
                let align = match value {
                    "center" => renderer::AlignContent::Center,
                    "flex-end" => renderer::AlignContent::FlexEnd,
                    // Taffy's Start/End ignore flex reversal. RTL rows use RowReverse. In columns, left/right
                    // behave as start.
                    "start" | "end" | "left" | "right" => {
                        if match value {
                            "left" => false,
                            "right" => inline,
                            _ => (value == "end") != (inline && rtl),
                        } {
                            renderer::AlignContent::End
                        } else {
                            renderer::AlignContent::Start
                        }
                    }
                    "space-between" => renderer::AlignContent::SpaceBetween,
                    "space-around" => renderer::AlignContent::SpaceAround,
                    "space-evenly" => renderer::AlignContent::SpaceEvenly,
                    "stretch" => renderer::AlignContent::Stretch,
                    _ => renderer::AlignContent::FlexStart,
                };
                if prop == Prop::JUSTIFY_CONTENT {
                    el.style().justify_content = Some(align);
                } else {
                    el.style().align_content = Some(align);
                }
            }
            Prop::TEXT_ALIGN => {
                el = match (value, rtl) {
                    ("center" | "-webkit-center", _) => el.text_center(),
                    ("right" | "-webkit-right", _)
                    | ("end", false)
                    | ("start" | "justify", true) => el.text_right(),
                    _ => el.text_left(),
                }
            }
            Prop::POSITION if matches!(value, "absolute" | "fixed") => el = el.absolute(),
            Prop::LEFT | Prop::RIGHT | Prop::TOP | Prop::BOTTOM
                if matches!(position, "relative" | "absolute") =>
            {
                // Like Chrome, a percent offset of a relative box against a parent of auto height is `auto`.
                let fraction = !(position == "relative"
                    && vertical
                    && slot.is_none()
                    && reader.parent(id, Prop::HEIGHT).is_none_or(|height| {
                        matches!(
                            height,
                            "auto" | "fit-content" | "min-content" | "max-content"
                        ) || height.contains('%')
                            && parent.is_none_or(|parent| doc.nodes[parent].parent != Some(0))
                    }));
                let length: renderer::Length = if let Some(percent) = percentage(value) {
                    if !fraction {
                        continue;
                    }
                    match block_size {
                        Some(_) => px(percent / 100. * base).into(),
                        None => relative(percent / 100.),
                    }
                } else if let Some((share, offset)) =
                    percent_calc(&units, value).filter(|_| block_size.is_none())
                {
                    if !fraction {
                        continue;
                    }
                    renderer::Length::Calc(share, px(offset))
                } else if let Some(length) = length() {
                    px(length).into()
                } else {
                    continue;
                };
                el = match prop {
                    Prop::LEFT => el.left(length),
                    Prop::RIGHT => el.right(length),
                    Prop::TOP => el.top(length),
                    _ => el.bottom(length),
                };
            }
            // An empty cell draws no background and borders.
            Prop::EMPTY_CELLS
                if value == "hide"
                    && style::role(styles, id) == style::CELL
                    && node.children.iter().all(|&child| {
                        doc.nodes[child].tag == "#text"
                            && doc.nodes[child].text.trim_ascii().is_empty()
                    }) =>
            {
                el.style().visibility = renderer::Visibility::Hidden
            }
            Prop::VISIBILITY => {
                el.style().visibility = if value == "hidden" || value == "collapse" {
                    renderer::Visibility::Hidden
                } else {
                    renderer::Visibility::Visible
                }
            }
            Prop::OPACITY if effect_layer.is_none() => el = el.opacity(opacity),
            Prop::WHITE_SPACE if matches!(value, "nowrap" | "pre") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::TEXT_WRAP if value.contains("nowrap") => {
                el = el.whitespace_nowrap().flex_nowrap()
            }
            Prop::WORD_BREAK | Prop::OVERFLOW_WRAP => {
                el.text_style().set_word_break(style::word_break(style));
            }
            Prop::TEXT_WRAP => {
                el.text_style().set_text_wrap(if value.contains("balance") {
                    renderer::TextWrap::Balance
                } else if value.contains("pretty") {
                    renderer::TextWrap::Pretty
                } else {
                    renderer::TextWrap::Wrap
                });
            }
            Prop::CURSOR => {
                if let Some(cursor) = style::cursor(value) {
                    el = el.cursor(cursor);
                }
            }
            _ => {}
        }
    }
    if let Some(natural) = natural {
        let chrome = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        let props = [Prop::WIDTH, Prop::HEIGHT];
        let hint = [0, 1].map(|axis| {
            node.attrs
                .get(["width", "height"][axis])
                .filter(|_| {
                    style.get(props[axis]).is_none()
                        && matches!(node.tag.as_str(), "img" | "video")
                        && !(broken && natural == [16., 16.])
                })
                .and_then(|value| dimension(value, false))
                .map(|length| length + extra[axis])
        });
        if let Some(width) = hint[0] {
            el = el.w(px(width));
        }
        if let Some(height) = hint[1] {
            el = el.h(px(height));
        }
        let set = [0, 1].map(|axis| {
            hint[axis].is_some()
                || style.get(props[axis]).is_some_and(|value| {
                    value != "auto"
                        && !(axis == 1 && line_base == Some(false) && value.contains('%'))
                })
        });
        let [content_width, content_height] = [0, 1].map(|axis| {
            hint[axis]
                .or_else(|| {
                    style
                        .get(props[axis])
                        .filter(|value| !value.contains('%'))
                        .and_then(|value| units.resolve(value, containing[axis]))
                        .map(|length| length + extra[axis])
                })
                .map(|length| (length - chrome[axis]).max(0.))
        });
        // A flex item's automatic minimum is the smaller of its specified and natural widths.
        if flex_parent
            && style
                .get(Prop::MIN_WIDTH)
                .is_none_or(|value| value == "auto")
            && reader
                .parent(id, Prop::FLEX_DIRECTION)
                .is_none_or(|value| !value.starts_with("column"))
            && let Some(percent) = style
                .get(Prop::WIDTH)
                .and_then(|value| value.strip_suffix('%'))
                .and_then(|value| value.parse::<f32>().ok())
        {
            el = el.min_w(px(
                (percent.max(0.) / 100. * containing[0] + extra[0]).min(natural[0] + chrome[0])
            ));
        }
        let stretch = [stretched(0), stretched(1)];
        let css = el.style().aspect_ratio.is_some();
        let line = (stretch[1]
            && reader
                .parent(id, Prop::FLEX_WRAP)
                .is_none_or(|value| value == "nowrap")
            && reader
                .parent(id, Prop::HEIGHT)
                .is_some_and(|value| value != "auto" && !value.contains('%')))
        .then(|| (containing[1] - margin[0] - margin[2] - chrome[1]).max(0.));
        let sized = !set[0] && !stretch[0] && !(css && set[1]);
        let width = if set[0] {
            content_width
        } else if !sized {
            None
        } else {
            match natural_ratio.filter(|_| !css) {
                Some(ratio) if set[1] => content_height.map(|height| height * ratio),
                Some(ratio) => Some(line.map_or(natural[0], |line| line * ratio)),
                None => Some(natural[0]),
            }
        };
        // Like Chrome, a video without a frame has no natural size when CSS gives it a ratio, so it fills the line.
        if sized
            && css
            && natural_ratio.is_none()
            && node.tag == "video"
            && !matches!(position, "absolute" | "fixed")
        {
            match el.style().aspect_ratio {
                Some(ratio) => {
                    // A box around it that shrinks to its content makes it as narrow as its frame.
                    let shrinks = shrinks_around(doc, styles, id);
                    reader.log(Query::Shrinks(id as u32), 0, hashed(shrinks));
                    let limit = |prop| {
                        style
                            .get(prop)
                            .and_then(|value| units.resolve(value, containing[0]))
                            .map(|value| value + extra[0])
                    };
                    let width = if shrinks {
                        chrome[0]
                    } else {
                        containing[0] - margin[1] - margin[3]
                    }
                    .min(limit(Prop::MAX_WIDTH).unwrap_or(f32::INFINITY))
                    .max(limit(Prop::MIN_WIDTH).unwrap_or(0.));
                    // The ratio of the width and height attributes is for the content box.
                    let height = if style.get(Prop::BOX_SIZING) == Some("border-box")
                        && style
                            .get(Prop::ASPECT_RATIO)
                            .is_some_and(|v| !v.contains("auto"))
                    {
                        width / ratio
                    } else {
                        (width - chrome[0]).max(0.) / ratio + chrome[1]
                    };
                    el = el.w(px(width)).h(px(height));
                }
                None => el = el.w(renderer::Length::Calc(1., px(-margin[1] - margin[3]))),
            }
        } else if sized {
            if let Some(width) = width {
                el = el.w(px(width + chrome[0]));
                if let Some(min) = floor(0, width + chrome[0]) {
                    el = el.min_w(px(min));
                }
            } else {
                el.style().aspect_ratio = natural_ratio;
                el = only_auto_ratio(el, style);
            }
        }
        if let Some(height) = match natural_ratio {
            _ if set[1] || line.is_some() || css => None,
            None => Some(natural[1]),
            Some(ratio) if matches!(position, "absolute" | "fixed") => {
                width.map(|width| width / ratio)
            }
            Some(_) => None,
        } {
            let height = height + chrome[1];
            if stretch[1]
                && style
                    .get(Prop::MIN_HEIGHT)
                    .is_none_or(|value| value == "auto")
            {
                el = el.min_h(px(height));
            } else {
                el = el.h(px(height));
                if let Some(min) = floor(1, height) {
                    el = el.min_h(px(min));
                }
            }
        }
    }
    if block_size.is_some() && natural.is_none() {
        for (axis, size, sides) in [(0, Prop::WIDTH, [3, 1]), (1, Prop::HEIGHT, [0, 2])] {
            let [start, end] = sides.map(|side| {
                style
                    .get([Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT][side])
                    .and_then(|value| units.resolve(value, containing[axis]))
            });
            if style.get(size).is_none_or(|value| value == "auto")
                && let (Some(start), Some(end)) = (start, end)
            {
                let length =
                    px(
                        (containing[axis] - start - end - margin[sides[0]] - margin[sides[1]])
                            .max(0.),
                    );
                el = if axis == 0 {
                    el.w(length)
                } else {
                    el.h(length)
                };
            }
        }
    }
    let replaced = natural.is_some()
        || matches!(
            node.tag.as_str(),
            "svg" | "input" | "button" | "select" | "textarea" | "progress" | "meter"
        );
    let auto = [Prop::MARGIN_LEFT, Prop::MARGIN_RIGHT]
        .map(|prop| style.get(prop).is_some_and(|value| value == "auto"));
    if hit.is_none()
        && auto.contains(&true)
        && !flex_parent
        && slot.is_none()
        && matches!(
            display,
            "block" | "flex" | "grid" | "list-item" | "flow-root"
        )
        && style.get(Prop::WIDTH).is_none_or(|value| value == "auto")
        && !matches!(position, "absolute" | "fixed")
        && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        && !replaced
    {
        if auto == [true, true] || margin[1] + margin[3] == 0. {
            el = el.w(relative(1.));
        } else {
            let style = el.style();
            (style.margin.left, style.margin.right) = (px(margin[3]).into(), px(margin[1]).into());
        }
    }
    if flex_parent
        && matches!(
            node.tag.as_str(),
            "img" | "video" | "canvas" | "iframe" | "svg" | "embed" | "object"
        )
        && [Prop::WIDTH, Prop::MAX_WIDTH]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value.contains('%')))
        && style
            .get(Prop::FLEX_BASIS)
            .is_none_or(|value| value == "auto")
        && node.parent.is_some()
        && reader
            .parent(id, Prop::FLEX_DIRECTION)
            .is_none_or(|value| value.starts_with("row"))
        && let width @ (renderer::Length::Px(_) | renderer::Length::Calc(..)) =
            el.style().size.width
    {
        el.style().flex_basis = width;
        el.style().size.width = renderer::Length::Auto;
    }
    if hit.is_none()
        && !flex_parent
        && slot.is_none()
        && !matches!(position, "absolute" | "fixed")
        && display.starts_with("inline")
        && let Some(parent) = node.parent
    {
        if display != "inline"
            && el.style().size.width == renderer::Length::Auto
            && (!replaced || node.tag == "button")
            && float_side(style).is_none()
        {
            el.style().flex_shrink = 1.;
        }
        reader.log(
            Query::ParentFont,
            depth,
            hashed(parent_font(doc, styles, id)),
        );
        let align = style.get(Prop::VERTICAL_ALIGN).unwrap_or("baseline");
        let middle = match el.style().size.height {
            _ if align != "middle" => None,
            _ if display == "inline" => Some(units.font.lh),
            renderer::Length::Px(height) => Some(f32::from(height) + margin[0] - margin[2]),
            _ => None,
        };
        let sized = matches!(el.style().size.height, renderer::Length::Px(_));
        // Like Chrome, a middle box puts the middle of its margin box half an x-height above the baseline.
        let centered = align == "middle" && middle.is_none();
        el.style().align_self = match align {
            "top" => Some(renderer::AlignItems::FlexStart),
            "text-top" if !sized && display != "inline" => Some(renderer::AlignItems::FlexStart),
            "bottom" => Some(renderer::AlignItems::FlexEnd),
            _ => None,
        };
        let below = |style: &css::properties::Style, font: css::value::units::FontUnits| {
            let face = text.face(&style::font(style));
            font.lh - f32::from(text.baseline(face, px(font.em), px(font.lh), pixel_ratio))
        };
        let line = styles.lengths[parent].font;
        let replaced = matches!(
            node.tag.as_str(),
            "img"
                | "svg"
                | "canvas"
                | "video"
                | "audio"
                | "iframe"
                | "embed"
                | "object"
                | "progress"
                | "meter"
                | "textarea"
        ) || check
            || display == "inline-block"
                && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| style.get(prop).is_some_and(|value| value != "visible"))
            || display != "inline"
                && !node.tag.starts_with('#')
                && !input.is_some_and(|kind| {
                    ["button", "submit", "reset", "file"]
                        .iter()
                        .any(|other| kind.eq_ignore_ascii_case(other))
                })
                && cx.blank();
        let replaced = replaced && alt.is_none();
        let field = editable && node.tag == "input";
        let swatch = native
            .as_ref()
            .is_some_and(|look| look.control == super::super::paint::control::Control::Color);
        let parent_style = &styles.values[parent];
        let parent = below(parent_style, line);
        let edge = el.style().align_self.is_none()
            && !centered
            && (matches!(align, "text-top" | "text-bottom")
                || middle.is_some()
                || swatch
                || replaced && !field
                || display == "inline-grid"
                    && !node.children.iter().any(|&child| in_flow(doc, info, child)));
        if centered {
            el.style().baseline = renderer::Baseline::Middle;
        } else if edge {
            el.style().baseline = renderer::Baseline::None;
        }
        let lift = if centered {
            line.ex / 2. + (margin[2] - margin[0]) / 2.
        } else if !edge {
            0.
        } else if matches!(align, "text-top" | "text-bottom") {
            let face = text.face(&style::font(parent_style));
            let [ascent, descent, _] = text
                .line_metrics(face, px(line.em * pixel_ratio))
                .map(|value| f32::from(value) / pixel_ratio);
            // The inline box of a span is as tall as its line height.
            match el.style().size.height {
                renderer::Length::Px(height) if align == "text-top" => {
                    ascent - margin[0] - f32::from(height)
                }
                _ if align == "text-top" => ascent - units.font.lh,
                _ => margin[2] - descent,
            }
        } else if let Some(height) = middle {
            line.ex / 2. - height / 2.
        } else if swatch {
            -padding[2] - borders[2] - 4.
        } else if node.tag == "button"
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
            })
        {
            -padding[2] - borders[2]
        } else if native.as_ref().is_some_and(|look| {
            // Like Chrome, a range sits on the bottom of its border box, a progress bar without its native look
            // on the bottom of its margin box.
            !look.plain || look.control == super::super::paint::control::Control::Range
        }) {
            0.
        } else {
            margin[2]
        };
        let inset = |prop| style.get(prop).filter(|value| *value != "auto");
        let offset = match (inset(Prop::TOP), inset(Prop::BOTTOM)) {
            _ if position != "relative" => Some(0.),
            (Some(top), _) => units.resolve(top, f32::NAN),
            (None, Some(bottom)) => units.resolve(bottom, f32::NAN).map(|bottom| -bottom),
            (None, None) => Some(0.),
        };
        let raise = match align {
            _ if offset.is_none() => 0.,
            "super" => line.em / 3. + 1. / pixel_ratio + lift,
            "sub" => -line.em / 5. - 1. / pixel_ratio + lift,
            "baseline" => lift,
            value => units.resolve(value, units.font.lh).unwrap_or(0.) + lift,
        };
        // A box with a line of text has its baseline there, also when the text overflows its height.
        let reserve = match (el.style().size.height, el.style().align_self) {
            (renderer::Length::Px(height), None) if display != "inline" && replaced => {
                line.lh - parent - f32::from(height)
            }
            (renderer::Length::Auto, None) if field => {
                line.lh - parent - units.font.lh - padding[0] - borders[0]
                    + below(style, units.font)
            }
            _ => f32::NEG_INFINITY,
        };
        let floor = margin[2].min(0.) + parent
            - match () {
                _ if centered
                    || matches!(display, "inline-block" | "inline-flex" | "inline-grid")
                        && !replaced =>
                {
                    f32::INFINITY
                }
                // A top or bottom box sits on a line edge, so the strut below the baseline does not reach it.
                _ if matches!(
                    el.style().align_self,
                    Some(renderer::AlignItems::FlexStart | renderer::AlignItems::FlexEnd)
                ) =>
                {
                    f32::INFINITY
                }
                _ if edge => 0.,
                _ if display == "inline" => below(style, units.font),
                _ => padding[2] + borders[2] + below(style, units.font),
            };
        if !crate::render::inline::makes_no_line(doc, styles, id)
            && (raise != 0. || reserve > margin[0] || margin[2] < floor)
        {
            el = el
                .mt(px((margin[0] + raise).max(reserve)))
                .mb(px((margin[2] - raise).max(floor)));
        }
        if let Some(offset) = offset
            && (raise != 0. || offset != 0.)
        {
            el = el.top(px(offset - raise));
        }
    }
    if let Some((color, content)) = clipped_fill {
        let inset = std::array::from_fn::<_, 4, _>(|side| {
            px(borders[side] + if content { padding[side] } else { 0. })
        });
        let mut fill = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let fill = Rect::between(
                    outer.origin + point(inset[3], inset[0]),
                    outer.bottom_right() - point(inset[1], inset[2]),
                );
                let corners = radius
                    .resolve(outer.size)
                    .inset(inset[0], inset[1], inset[2], inset[3])
                    .fit(fill.size);
                window.draw_box(fill, corners, color, renderer::Border::default());
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        if boxed {
            fill.style().escape = renderer::Escape::OUTLINE;
        }
        el = el.child(fill);
    }
    let layers = canvas.map_or(style, |source| &styles.values[source]);
    let mut background = Vec::new();
    if let Some(color) = text_fill {
        background.push(
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let outer = Rect::between(
                        bounds.origin - point(px(borders[3]), px(borders[0])),
                        bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                    );
                    window.draw_box(
                        outer,
                        radius.resolve(outer.size),
                        color,
                        renderer::Border::default(),
                    );
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into(),
        );
    }
    if !propagated
        && layers
            .get(Prop::BACKGROUND_IMAGE)
            .is_some_and(|value| value != "none")
    {
        let base = view
            .host
            .borrow()
            .page_state(view.id)
            .and_then(|state| state.base.clone());
        background.extend(css_background(
            layers,
            canvas.map_or(units, |source| styles.lengths[source]),
            id,
            0,
            if canvas.is_some() {
                Default::default()
            } else {
                radius
            },
            [borders, padding, [0.; 4]],
            view.clone(),
            base.as_ref(),
            &view.host.borrow().app.assets,
            canvas.is_some(),
        ));
    }
    // As in Chrome, the background of a scroll container doesn't scroll with the content or add to its scroll
    // range, unless it is `local`.
    if boxed
        && !text_clip
        && !layers
            .get(Prop::BACKGROUND_ATTACHMENT)
            .is_some_and(|value| value.contains("local"))
    {
        for layer in &mut background {
            if let Child::Node(node) = layer {
                node.style().escape = renderer::Escape::OUTLINE;
            }
        }
    }
    if text_clip {
        el = el.child(
            renderer::node()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(background)
                .hooks(super::super::paint::Effected {
                    effect: renderer::PaintEffect {
                        text_mask: true,
                        ..Default::default()
                    },
                    backdrop: None,
                }),
        );
    } else {
        el = el.children(background);
    }
    if !inner_shadows.is_empty() {
        el = el.child(inset_shadows(inner_shadows, radius, borders));
    }
    el = el.children(border_image);
    el = el.children(field_layer);
    el = el.children(content_layers);
    if let Some(fit) = picture {
        let inset = |side: usize| px(borders[side] + padding[side]);
        let corners = radius
            .to_pixels(size(px(basis()[0]), px(basis()[1])))
            .inset(inset(0), inset(1), inset(2), inset(3));
        let cropped = matches!(fit, renderer::ObjectFit::Cover | renderer::ObjectFit::None);
        let mut image = renderer::node()
            .size_full()
            .hooks(super::super::paint::Picture {
                id,
                view: view.clone(),
                source: super::super::paint::Source::Element,
                fit,
                cropped: cropped.then_some(corners),
                position: style
                    .get(Prop::OBJECT_POSITION)
                    .filter(|value| !matches!(*value, "50% 50%" | "center" | "center center"))
                    .map(|value| (value.into(), units)),
                pixelated: matches!(
                    style.get(Prop::IMAGE_RENDERING),
                    Some("pixelated" | "crisp-edges")
                ),
            });
        image.style().aspect_ratio = natural_ratio;
        if !cropped {
            image.style().corner_radii = renderer::Radii {
                x: corners.x.map(renderer::Length::Px),
                y: corners.y.map(renderer::Length::Px),
                shape: corners.shape,
            };
        }
        el = el.child(image);
    }
    if let Some(lines) = style
        .get(Prop::WEBKIT_LINE_CLAMP)
        .or_else(|| style.get(Prop::LINE_CLAMP))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|_| hit.is_none())
    {
        let text = el.text_style();
        text.set_white_space(renderer::WhiteSpace::Normal);
        text.set_line_clamp(Some(lines.max(1)));
        text.set_text_overflow(Some("…".into()));
    } else if hit.is_none()
        && style.get(Prop::TEXT_OVERFLOW) == Some("ellipsis")
        && matches!(style.get(Prop::WHITE_SPACE), Some("nowrap" | "pre"))
        && el.style_ref().overflow.x != renderer::Overflow::Visible
    {
        el.text_style().set_text_overflow(Some("…".into()));
    }
    if position == "fixed" {
        let viewport = units.viewport;
        let inset = [Prop::LEFT, Prop::RIGHT, Prop::TOP, Prop::BOTTOM].map(|prop| {
            style.get(prop).and_then(|v| {
                units.resolve(
                    v,
                    viewport[usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM))],
                )
            })
        });
        if style.get(Prop::WIDTH).is_none_or(|v| v == "auto")
            && let [Some(left), Some(right), ..] = inset
        {
            el = el.w(px((viewport[0] - left - right).max(0.)));
        }
        if style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && let [_, _, Some(top), Some(bottom)] = inset
        {
            el = el.h(px((viewport[1] - top - bottom).max(0.)));
        }
    }
    if style.get(Prop::OUTLINE_STYLE) == Some("auto") {
        // Like Chrome, a focus ring ignores the outline width: a 2px ring of the outline color, centered on the border
        // edge moved out by `outline-offset`, or 1px inside it with a border, above a 1px ring outside it of white, or
        // of #101010 in a dark scheme. Its corners follow the border radius, or else have a radius of 3px.
        let offset = style
            .get(Prop::OUTLINE_OFFSET)
            .and_then(|v| units.resolve(v, containing[0]))
            .unwrap_or(0.)
            .trunc()
            - if borders.iter().all(|width| *width >= 1.) {
                1.
            } else {
                0.
            };
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(ink);
        let under = renderer::Color::hex(if dark() { 0x101010ff } else { 0xffffffff });
        let mut ring = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size).fit(outer.size);
                let corners = if corners.is_zero() {
                    renderer::Radii::from(px(3.))
                } else {
                    let grown = |radius: renderer::Pixels| (radius + px(offset)).max(px(0.));
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    }
                };
                for (at, color) in [(offset + 1., under), (offset, color)] {
                    let path = Rect::between(
                        outer.origin - point(px(at), px(at)),
                        outer.bottom_right() + point(px(at), px(at)),
                    );
                    if path.size.width <= px(0.) || path.size.height <= px(0.) {
                        continue;
                    }
                    let rect = Rect::between(
                        path.origin - point(px(1.), px(1.)),
                        path.bottom_right() + point(px(1.), px(1.)),
                    );
                    let corners = corners.inset(px(-1.), px(-1.), px(-1.), px(-1.));
                    window.draw_box(
                        rect,
                        corners.fit(rect.size),
                        renderer::Color::TRANSPARENT,
                        renderer::Border {
                            widths: renderer::Edges::all(px(2.)),
                            color,
                            style: renderer::BorderStyle::Solid,
                            sides: 0,
                        },
                    );
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        ring.style().escape = renderer::Escape::OUTLINE;
        el = el.child(ring);
    } else if let Some(width) = style
        .get(Prop::OUTLINE_STYLE)
        .filter(|kind| !matches!(*kind, "none" | "hidden"))
        .and_then(|_| {
            units.resolve(
                style.get(Prop::OUTLINE_WIDTH).unwrap_or("3px"),
                containing[0],
            )
        })
        .filter(|width| *width > 0.)
    {
        let grow = width
            + style
                .get(Prop::OUTLINE_OFFSET)
                .and_then(|v| units.resolve(v, containing[0]))
                .unwrap_or(0.);
        let color = style
            .get(Prop::OUTLINE_COLOR)
            .and_then(css::value::color::paint)
            .unwrap_or(ink);
        let name = style.get(Prop::OUTLINE_STYLE).unwrap_or("solid");
        let kind = super::super::effects::border_style(name);
        // The top and left sides, then the right and bottom ones, of each band from the outer edge in.
        let bands: Vec<[renderer::Color; 2]> = match name {
            "inset" => vec![[true, false].map(|dark| shade_border(color, dark))],
            "outset" => vec![[false, true].map(|dark| shade_border(color, dark))],
            "groove" => vec![
                [true, false].map(|dark| shade_border(color, dark)),
                [false, true].map(|dark| shade_border(color, dark)),
            ],
            "ridge" => vec![
                [false, true].map(|dark| shade_border(color, dark)),
                [true, false].map(|dark| shade_border(color, dark)),
            ],
            _ => vec![[color; 2]],
        };
        // Like the border box, the outline is square at corners without a radius.
        let mut outline = renderer::canvas(
            |_, _| (),
            move |bounds, _, window| {
                let outer = Rect::between(
                    bounds.origin - point(px(borders[3]), px(borders[0])),
                    bounds.bottom_right() + point(px(borders[1]), px(borders[2])),
                );
                let corners = radius.resolve(outer.size);
                let grown = |radius: renderer::Pixels| {
                    if radius > px(0.) {
                        (radius + px(grow)).max(px(0.))
                    } else {
                        radius
                    }
                };
                let corners = corners.moved_out(
                    px(grow),
                    renderer::Radii {
                        x: corners.x.map(grown),
                        y: corners.y.map(grown),
                        shape: corners.shape,
                    },
                );
                let band = width / bands.len() as f32;
                for (at, colors) in bands.iter().enumerate() {
                    let inset = px(band * at as f32 - grow);
                    let rect = Rect::between(
                        outer.origin + point(inset, inset),
                        outer.bottom_right() - point(inset, inset),
                    );
                    let inner = px(band * at as f32);
                    let corners = corners.inset(inner, inner, inner, inner).fit(rect.size);
                    let split = colors[0] != colors[1];
                    for (sides, color) in [(if split { 9 } else { 0 }, colors[0]), (6, colors[1])]
                        .into_iter()
                        .take(1 + usize::from(split))
                    {
                        window.draw_box(
                            rect,
                            corners,
                            renderer::Color::TRANSPARENT,
                            renderer::Border {
                                widths: renderer::Edges::all(px(band)),
                                color,
                                style: kind,
                                sides,
                            },
                        );
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        // The outline takes the clips of its box, not the overflow clip of the box itself.
        outline.style().escape = renderer::Escape::OUTLINE;
        el = el.child(outline);
    }
    if shaded
        || first.is_some_and(|first| {
            visible
                .clone()
                .any(|side| border_colors[side] != border_colors[first])
        })
    {
        let mut bands = vec![([0.; 4], borders, [renderer::Color::TRANSPARENT; 4])];
        let split = (0..4).any(|side| matches!(border_kinds[side], "groove" | "ridge"));
        if split {
            bands.push(([0.; 4], [0.; 4], [renderer::Color::TRANSPARENT; 4]));
        }
        for side in 0..4 {
            let color = border_colors[side];
            let raised = side == 0 || side == 3;
            let (outer, inner) = match border_kinds[side] {
                "inset" => (shade_border(color, raised), None),
                "outset" => (shade_border(color, !raised), None),
                "groove" => (
                    shade_border(color, raised),
                    Some(shade_border(color, !raised)),
                ),
                "ridge" => (
                    shade_border(color, !raised),
                    Some(shade_border(color, raised)),
                ),
                _ => (color, None),
            };
            bands[0].2[side] = outer;
            if let Some(inner) = inner {
                let half = borders[side] / 2.;
                bands[0].1[side] = half;
                bands[1].0[side] = half;
                bands[1].1[side] = half;
                bands[1].2[side] = inner;
            }
        }
        if split {
            bands[1].0 = std::array::from_fn(|side| {
                if bands[1].1[side] > 0. {
                    bands[1].0[side]
                } else {
                    borders[side]
                }
            });
        }
        el = el.child(border_canvas(
            bands,
            borders,
            radius,
            border_kinds.map(super::super::effects::border_style),
            false,
        ));
    }
    let resizer = (node.tag == "textarea"
        && style.get(Prop::RESIZE).is_none_or(|value| value != "none"))
    .then_some(super::super::paint::control::Look {
        control: super::super::paint::control::Control::Resizer,
        ..Default::default()
    });
    if let Some(look) = native
        .filter(|look| {
            !matches!(
                look.control,
                super::super::paint::control::Control::Button
                    | super::super::paint::control::Control::Field
            )
        })
        .or(resizer)
    {
        el = el.child(if look.plain {
            let parts = styles.scrollbars.get(&id);
            let range = look.control == super::super::paint::control::Control::Range;
            let ink = css::value::color::rgba(
                style
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .unwrap_or((0, 255)),
            );
            // Size, margin-top, border width, radius, fill and border color of the track and thumb, or of the
            // bar and value of a progress bar, with the defaults of Chrome. Both have `box-sizing: border-box`.
            let [track, thumb] = [1, 2].map(|slot| {
                let part = parts.and_then(|parts| parts[slot].as_ref());
                let get = |prop| part.and_then(|part| part.get(prop));
                let length = |prop| get(prop).and_then(|value| units.resolve(value, f32::NAN));
                let color = |prop| get(prop).and_then(css::value::color::paint);
                let native = range
                    && slot == 2
                    && [Prop::APPEARANCE, Prop::WEBKIT_APPEARANCE]
                        .into_iter()
                        .all(|prop| get(prop).is_none_or(|value| value != "none"));
                let border = get(Prop::BORDER_TOP_STYLE)
                    .is_some_and(|value| !matches!(value, "none" | "hidden"))
                    .then(|| length(Prop::BORDER_TOP_WIDTH))
                    .flatten()
                    .unwrap_or(0.);
                let fill = match (range, slot) {
                    _ if native => Some(css::value::color::rgba((look.accent, 255))),
                    (false, 1) => Some(renderer::Color::hex(0x808080ff)),
                    (false, _) => Some(renderer::Color::hex(0x008000ff)),
                    _ => None,
                };
                (
                    [Prop::WIDTH, Prop::HEIGHT].map(|prop| length(prop).or(native.then_some(16.))),
                    length(Prop::MARGIN_TOP).unwrap_or(0.),
                    border,
                    if native {
                        8.
                    } else {
                        get(Prop::BORDER_TOP_LEFT_RADIUS).map_or(0., |value| {
                            // Only round ends matter here, and the box clamps large radii.
                            if value.contains('%') {
                                1e4
                            } else {
                                units.resolve(value, f32::NAN).unwrap_or(0.)
                            }
                        })
                    },
                    color(Prop::BACKGROUND_COLOR).or(fill),
                    color(Prop::BORDER_TOP_COLOR).unwrap_or(ink),
                )
            });
            let (value, inset) = (look.value, padding);
            renderer::canvas(
                |_, _| (),
                move |bounds, _, window| {
                    let content = Rect::between(
                        bounds.origin + point(px(inset[3]), px(inset[0])),
                        bounds.bottom_right() - point(px(inset[1]), px(inset[2])),
                    );
                    let [width, height] = [content.size.width, content.size.height].map(f32::from);
                    let thumb_size = thumb.0.map(|side| side.unwrap_or(0.));
                    let track_height = match track.0[1] {
                        Some(side) if range => side,
                        _ if range => thumb_size[1] + 2. * track.2,
                        _ => height,
                    };
                    let track_top = (height - track_height) / 2.;
                    let inner = width - 2. * track.2;
                    let rects = [
                        (track, [0., track_top, width, track_height]),
                        if range {
                            (
                                thumb,
                                [
                                    track.2 + (inner - thumb_size[0]) * value,
                                    track_top + track.2 + thumb.1,
                                    thumb_size[0],
                                    thumb_size[1],
                                ],
                            )
                        } else {
                            (
                                thumb,
                                [
                                    track.2,
                                    track.2,
                                    inner * value.max(0.),
                                    height - 2. * track.2,
                                ],
                            )
                        },
                    ];
                    for ((_, _, border, radius, fill, edge), [x, y, w, h]) in rects {
                        if w <= 0. || h <= 0. {
                            continue;
                        }
                        let rect =
                            Rect::new(content.origin + point(px(x), px(y)), size(px(w), px(h)));
                        window.draw_box(
                            rect,
                            renderer::Radii::from(px(radius)).fit(rect.size),
                            fill.unwrap_or(renderer::Color::TRANSPARENT),
                            renderer::Border {
                                widths: px(border).into(),
                                color: edge,
                                style: renderer::BorderStyle::Solid,
                                sides: 0,
                            },
                        );
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into()
        } else {
            super::super::paint::control::layer(id, look, view.clone())
        });
    }
    let overscroll = overscroll(style);
    let handle = {
        let mut host = view.host.borrow_mut();
        let scroll = &mut host.state(view.id).geometry.scroll;
        if scrolls.contains(&true)
            || !page
                && overflow.contains(&renderer::Overflow::Hidden)
                && overscroll != renderer::Point::default()
        {
            Some(scroll.entry(id).or_default().clone())
        } else if overflow.contains(&renderer::Overflow::Hidden) {
            reader.log(
                Query::Scroll(id as u32),
                0,
                hashed(scrolled(scroll.get(&id))),
            );
            scroll.get(&id).cloned()
        } else {
            None
        }
    };
    let mut bars = None;
    if let Some(handle) = handle {
        if !editable {
            el = el.track_scroll(&handle);
            if hit.is_none() {
                el.style().restrict_scroll_to_axis = true;
                el.style().overscroll = overscroll;
            }
        }
        bars = Some(scrollbar::Scrollbar {
            id,
            handle,
            look,
            axes: scrolls,
            reserved,
            borders,
            origin: [
                padding[3] - borders[1] - borders[3],
                padding[0] - borders[0] - borders[2],
            ],
            quiet: false,
        });
    }
    let bars = grid_bar.or(bars);
    let marker_style = styles
        .text_pseudos
        .get(&id)
        .and_then(|pseudos| pseudos[0].as_ref());
    let content = marker_style
        .and_then(|marker| marker.get(Prop::CONTENT))
        .filter(|value| *value != "normal");
    if display == "list-item"
        && content.is_none_or(|value| value != "none")
        && let Some(kind) = style
            .get(Prop::LIST_STYLE_TYPE)
            .filter(|kind| *kind != "none" || content.is_some())
    {
        let list = node.parent.map(|parent| &doc.nodes[parent]);
        let items = node.parent.map(|parent| {
            siblings
                .borrow_mut()
                .entry(parent)
                .or_insert_with(|| {
                    let items = doc.nodes[parent].children.iter().copied().filter(|&child| {
                        styles.values[child]
                            .get(Prop::DISPLAY)
                            .is_some_and(|value| value == "list-item")
                    });
                    Rc::new(
                        items
                            .enumerate()
                            .map(|(index, item)| (item, index))
                            .collect(),
                    )
                })
                .clone()
        });
        let position = items
            .as_ref()
            .and_then(|items| items.get(&id).copied())
            .unwrap_or(0) as i64;
        let count = items.as_ref().map_or(1, |items| items.len()) as i64;
        let start = list
            .and_then(|list| list.attrs.get("start"))
            .and_then(|value| value.parse::<i64>().ok());
        let number = node
            .attrs
            .get("value")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(
                if list.is_some_and(|list| list.attrs.contains_key("reversed")) {
                    start.unwrap_or(count) - position
                } else {
                    start.unwrap_or(1) + position
                },
            );
        let number = styles.list_items.get(&id).copied().unwrap_or(number);
        let marker = match kind {
            "disc" | "circle" | "square" => css::value::counter::counter_text(kind, number),
            "disclosure-closed" => "▸".to_owned(),
            "disclosure-open" => "▾".to_owned(),
            other if other.starts_with(['"', '\'']) => other.trim_matches(['"', '\'']).to_owned(),
            "cjk-decimal"
            | "cjk-ideographic"
            | "trad-chinese-informal"
            | "simp-chinese-informal"
            | "hiragana"
            | "katakana" => format!("{}、", css::value::counter::counter_text(kind, number)),
            _ => format!("{}.", css::value::counter::counter_text(kind, number)),
        };
        let marker = content.map_or(marker, |content| {
            let mut text = String::new();
            for part in css::value::tokens::components(content, ' ') {
                if part.starts_with("counter(") || part.starts_with("counters(") {
                    text.push_str(&number.to_string());
                    continue;
                }
                let mut input = cssparser::Parser::new(part);
                if let Ok(string) = input.expect_string() {
                    text.push_str(string);
                }
            }
            text
        });
        let inside = style
            .get(Prop::LIST_STYLE_POSITION)
            .is_some_and(|value| value == "inside");
        let (indent, percent) = text_indent(style, &units);
        let indent = indent + percent * containing[0];
        let face = text.face(&style::font(marker_style.unwrap_or(style)));
        let size = marker_style
            .and_then(|marker| marker.get(Prop::FONT_SIZE))
            .and_then(|value| units.resolve(value, units.font.em))
            .unwrap_or(units.font.em);
        let metrics = text
            .line_metrics(face, px(size * pixel_ratio))
            .map(f32::from);
        let line = match marker_style.map(style::line_height) {
            Some(style::LineHeight::Fixed(line)) => line,
            Some(style::LineHeight::Scale(scale)) => scale * size,
            Some(style::LineHeight::Normal) => metrics.iter().sum::<f32>() / pixel_ratio,
            None => units.font.lh,
        };
        let mut top = padding[0];
        if !inside && marker_style.is_some() {
            let own = text.face(&parsed.face);
            let rise = f32::from(text.baseline(face, px(size), px(line), pixel_ratio))
                - f32::from(text.baseline(own, px(units.font.em), px(units.font.lh), pixel_ratio));
            if rise > 0. {
                el = el.pt(px(padding[0] + rise));
            } else {
                top -= rise;
            }
        }
        let mut marker_box = if content.is_none() && kind.starts_with("disclosure-") {
            let [ascent, descent, _] = metrics;
            let side = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let space = text
                .advance(face, px(size), ' ')
                .map_or(size / 4., f32::from);
            let look = super::super::paint::control::Look {
                control: super::super::paint::control::Control::Disclosure,
                checked: kind == "disclosure-open",
                ink: marker_style
                    .unwrap_or(style)
                    .get(Prop::COLOR)
                    .and_then(css::value::color::color)
                    .map_or(0, |(ink, _)| ink),
                ..Default::default()
            };
            let triangle = renderer::node()
                .absolute()
                .left_0()
                .top(px((leading + ascent - side) / pixel_ratio))
                .size(px(side / pixel_ratio))
                .child(super::super::paint::control::layer(id, look, view.clone()));
            let above = (leading + ascent) / pixel_ratio;
            let marker = renderer::node()
                .flex_shrink_0()
                .w(px(side / pixel_ratio + space))
                .h(px(above))
                .mb(px(line - above))
                .baseline(renderer::Baseline::Bottom)
                .child(triangle);
            if inside {
                marker
            } else {
                marker.absolute().top(px(top)).right(relative(1.))
            }
        } else if inside && !(content.is_none() && matches!(kind, "disc" | "circle" | "square")) {
            // The marker starts the first line, so the text of that line is indented by its width.
            let marker = if marker.ends_with(' ') {
                marker
            } else {
                marker + " "
            };
            let run = style::text_run(marker_style.unwrap_or(style), &units, marker.len());
            let width = f32::from(text.shape_line(&marker, px(size), &run).line.width);
            el.text_style()
                .set_text_indent(px(indent + width))
                .set_text_indent_percent(0.);
            renderer::node()
                .absolute()
                .top(px(top))
                .left(px(padding[3] + indent))
                .whitespace_nowrap()
                .child(marker)
        } else if content.is_none() && matches!(kind, "disc" | "circle" | "square") {
            let color = marker_style
                .unwrap_or(style)
                .get(Prop::COLOR)
                .and_then(css::value::color::paint)
                .unwrap_or(Color::hex(0x000000ff));
            let [ascent, descent, _] = metrics;
            let offset = (ascent * 2. / 3.).floor();
            let leading = ((line * pixel_ratio - ascent - descent) / 2.).floor();
            let side = ((offset + 1.) / 2.).floor() / pixel_ratio;
            // Like Chrome, an inside symbol takes its size, 1px and 1em before the text.
            if inside {
                el.text_style()
                    .set_text_indent(px(indent + side + 1. + size))
                    .set_text_indent_percent(0.);
            }
            let dot = renderer::node()
                .absolute()
                .left(px(if inside {
                    padding[3] + indent
                } else {
                    padding[3] - (offset + 7.) / pixel_ratio
                }))
                .top(px(top
                    + (leading + (3. * (ascent - offset) / 2.).floor())
                        / pixel_ratio))
                .size(px(side));
            match kind {
                "disc" => dot.rounded_full().bg(color),
                "circle" => dot
                    .rounded_full()
                    .border(px(1. / pixel_ratio))
                    .border_color(color),
                _ => dot.bg(color),
            }
        } else {
            renderer::node()
                .absolute()
                .top(px(top))
                .right(relative(1.))
                // Like Chrome, a counter marker ends with a space, and strings and `、` end as they are.
                .pr(px(
                    if content.is_some()
                        || marker.ends_with(['、', ' '])
                        || kind.starts_with(['"', '\''])
                    {
                        0.
                    } else {
                        text.advance(face, px(size), ' ')
                            .map_or(size / 4., f32::from)
                    },
                ))
                .whitespace_nowrap()
                .child(marker)
        };
        if let Some(marker) = marker_style {
            let face = style::font(marker);
            marker_box = marker_box
                .font_family(face.family)
                .font_weight(face.weight)
                .text_size(px(size))
                .line_height(px(line));
            marker_box
                .text_style()
                .set_font_style(face.style)
                .set_font_synthesis(face.synthesis);
            if let Some(ink) = marker.get(Prop::COLOR).and_then(css::value::color::color) {
                marker_box = marker_box.text_color(css::value::color::rgba(ink));
            }
        }
        // Like Chrome's `::marker`, numbers take tabular widths.
        marker_box
            .text_style()
            .set_font_features(Rc::from([("tnum".to_owned(), 1)]));
        // Like Chrome, an empty item keeps the line of its marker.
        if !inside
            && node.children.iter().all(|&child| {
                doc.nodes[child].tag == "#text" && doc.nodes[child].text.trim_ascii().is_empty()
            })
            && style.get(Prop::HEIGHT).is_none_or(|v| v == "auto")
            && style.get(Prop::MIN_HEIGHT).is_none_or(|v| v == "auto")
        {
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            el = el.min_h(px(line.max(units.font.lh) + edges));
        }
        el = el.child(marker_box);
    }
    let fr = Frame {
        padding,
        borders,
        margin,
        extra,
        intrinsic_width,
        inset_x,
        natural_ratio,
    };
    let content_box = || fr.content_box(&cx);
    let kind = style.get(Prop::CONTAINER_TYPE);
    let own = kind
        .is_some_and(|v| matches!(v, "inline-size" | "size"))
        .then(|| {
            let mut host = view.host.borrow_mut();
            let dom = host.state(view.id);
            let last = dom.layout.cull.widths.get(&id).copied();
            reader.log(
                Query::Width(id as u32),
                0,
                hashed(last.map(|size| size.map(f32::to_bits))),
            );
            let size = last.unwrap_or_else(content_box);
            dom.layout.containers.insert(id, size);
            // Block-axis units of an inline-size container use the next size container, or the viewport.
            if kind == Some("inline-size") {
                [size[0], container[1]]
            } else {
                size
            }
        });
    if let Some(pseudos) = styles.pseudos.get(&id).filter(|_| !skipped) {
        el = match pseudo_elements(&cx, &fr, el, &mut carry, pseudos, own) {
            ControlFlow::Continue(el) => el,
            ControlFlow::Break(done) => return done,
        };
    }
    let preserve = style
        .get(Prop::WHITE_SPACE)
        .is_some_and(|v| matches!(v, "pre" | "pre-wrap" | "break-spaces"))
        || node.tag == "pre";
    if hit.is_none()
        && (node.tag == "#text" || display == "inline" && !flex_parent && slot.is_none())
        && style
            .get(Prop::WHITE_SPACE)
            .is_none_or(|value| !matches!(value, "pre" | "nowrap"))
    {
        el = el.min_w(px(0.)).max_w(relative(1.)).flex_shrink(1.);
    }
    if preserve && node.text == "\n" {
        el = el.min_h(px(units.font.lh));
    }
    let inline = if skipped
        || editable
        || matches!(node.tag.as_str(), "svg" | "canvas" | "iframe" | "terminal")
    {
        None
    } else {
        cx.inline_text().filter(|data| {
            !data.text.is_empty() || {
                let found = content_editable(doc, id);
                reader.log(
                    Query::Editable,
                    found.map_or(1, |node| reader.scope_of(id, node)),
                    hashed(found.is_some()),
                );
                found.is_some()
            }
        })
    };
    let hit_test = inline.is_some();
    let drag = style
        .get(Prop::APP_REGION)
        .is_some_and(|value| value == "drag");
    if hit.is_none()
        && (hit_test || node.tag == "#text")
        && !drag
        && {
            // Read the parent even with an explicit mode: an ancestor exit can set display: none without a
            // new revision.
            let (mode, origin) = styles.user_select[id];
            let origin = if origin == id {
                node.parent.unwrap_or(id)
            } else {
                origin
            };
            reader.log(
                Query::UserSelect,
                reader.scope_of(id, origin),
                hashed(mode as u8),
            );
            mode != style::UserSelect::None
        }
        && style.get(Prop::CURSOR).is_none_or(|value| value == "auto")
    {
        el = el.cursor(renderer::Cursor::Text);
    }
    let mut plans = 0;
    let cell = slot.is_some() && style::role(styles, id) == style::CELL;
    // As in browsers, a button centers its content in its height, and `align-content` aligns it in a block.
    // Content that overflows starts at the top, unless the value says `unsafe`.
    let content = match style.get(Prop::ALIGN_CONTENT) {
        _ if !flow || cell => None,
        _ if node.tag == "button" => Some(renderer::AlignContent::Center),
        _ if display == "inline" => None,
        Some(value) if value.ends_with("center") || value.starts_with("space-") => {
            (value != "space-between").then_some(renderer::AlignContent::Center)
        }
        Some(value) if value.ends_with("end") => Some(renderer::AlignContent::FlexEnd),
        _ => None,
    };
    if let Some(data) = inline {
        el = paragraph(&cx, &fr, el, &mut carry, data, content);
    } else if drawable {
        // The children of a canvas with drawable content lay out as blocks at its content box, and take no room
        // in the canvas. The window doesn't show them: the page draws them with `drawElementImage()`.
        let mut content = content_box();
        for axis in 0..2 {
            if style
                .get([Prop::WIDTH, Prop::HEIGHT][axis])
                .is_none_or(|value| value == "auto")
                && let Some(natural) = natural
            {
                content[axis] = natural[axis];
            }
        }
        let boxes = flat
            .iter()
            .map(|&child| element(build, child, content, container, None));
        let holder = renderer::node()
            .absolute()
            .flex()
            .flex_col()
            .top(px(padding[0]))
            .right(px(padding[1]))
            .left(px(padding[3]))
            .children(boxes);
        el = el.child(holder.hooks(super::super::paint::Undrawn(view.clone())));
    } else if (!node.children.is_empty() || carry.items.iter().any(Option::is_some))
        && !skipped
        && node.tag != "canvas"
        && node.tag != "terminal"
        && node.tag != "iframe"
        && node.tag != "svg"
        && !editable
    {
        el = children(
            &cx,
            &fr,
            el,
            &mut carry,
            own,
            preserve,
            &mut cut_parts,
            &mut plans,
        );
    }
    if let Some([width, height]) = intrinsic.filter(|size| skipped && size != &[0.; 2]) {
        el = el.child(renderer::node().w(px(width)).h(px(height)).flex_shrink_0());
    }
    if let Some([_, height]) = intrinsic.filter(|_| forced) {
        let sides = [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ];
        // The last frame laid out the box without its contents. Before one, only the height is known.
        let size = last.map_or([None, Some(height)], |size| size.map(Some));
        let style = el.style();
        for (axis, value) in size.into_iter().enumerate() {
            if let Some(value) = value {
                let value = renderer::Length::Px(px(value + sides[axis]));
                let [size, min, max] = if axis == 0 {
                    [
                        &mut style.size.width,
                        &mut style.min_size.width,
                        &mut style.max_size.width,
                    ]
                } else {
                    [
                        &mut style.size.height,
                        &mut style.min_size.height,
                        &mut style.max_size.height,
                    ]
                };
                (*size, *min, *max) = (value, value, value);
            }
        }
    }
    if info[id].flags & cull::SKIPS != 0 && style.get(Prop::CONTENT_VISIBILITY) == Some("auto") {
        let clips = ancestors
            .iter()
            .copied()
            .filter(|&ancestor| {
                !matches!(doc.nodes[ancestor].tag.as_str(), "html" | "body")
                    && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                        .into_iter()
                        .any(|prop| {
                            styles.values[ancestor]
                                .get(prop)
                                .is_some_and(|value| value != "visible")
                        })
            })
            .collect();
        if let Some(dom) = view.host.borrow_mut().page_state_mut(view.id) {
            let last = dom
                .layout
                .autos
                .get(&id)
                .map_or((0, [f32::NAN; 2]), |entry| entry.2);
            dom.layout.autos.insert(id, (chain.clone(), clips, last));
        }
    }
    carry.after.extend(
        carry
            .items
            .into_iter()
            .flatten()
            .map(|(order, decoration, _)| (order, decoration)),
    );
    carry.after.sort_by_key(|(order, _)| *order);
    el = el.children(carry.after.into_iter().map(|(_, decoration)| decoration));
    if hit.is_none() && (cell && flow || content.is_some()) {
        el.style().safe = node.tag == "button"
            || !cell
                && style
                    .get(Prop::ALIGN_CONTENT)
                    .is_some_and(|value| !value.starts_with("unsafe"));
        let align = match style.get(Prop::VERTICAL_ALIGN) {
            _ if !cell => content,
            Some("middle") => Some(renderer::AlignContent::Center),
            Some("bottom") => Some(renderer::AlignContent::FlexEnd),
            _ => None,
        };
        if all_inline {
            el.style().align_content = align.or(Some(renderer::AlignContent::FlexStart));
        } else {
            el.style().justify_content = align;
        }
    }
    if let Some((_, grid)) = &slot {
        el.style().baseline_source = grid.first_item.is_some();
        el = el
            .absolute()
            .mt(px(margin[0]))
            .mr(px(margin[1]))
            .mb(px(margin[2]))
            .ml(px(margin[3]));
    }
    // Windows moves the window in the native hit test (`window_drag_area`), macOS and Linux on the first drag.
    if drag {
        el = el.window_drag_area().on_pointer({
            let view = view.clone();
            move |event, window, cx| {
                let renderer::Event::PointerDown {
                    at: position,
                    button: renderer::Button::Left,
                    clicks,
                    ..
                } = *event
                else {
                    return;
                };
                // Use the nearest drag-region box under the pointer so no-drag children keep their clicks.
                let dragged = view.host.try_borrow().is_ok_and(|host| {
                    let viewport = *host.viewports.get(&view.id).unwrap_or(&[800., 600.]);
                    let Some(page) = host.pages.get(&view.id) else {
                        return true;
                    };
                    let state = &page.state;
                    let Some(computed) = &state.styles.computed else {
                        return true;
                    };
                    let at = [position.x, position.y].map(f32::from);
                    crate::render::hit::hit_test(
                        &page.doc, state, computed, viewport, at, true, false,
                    )
                    .as_u64()
                    .is_none_or(|target| {
                        std::iter::successors(Some(target as usize), |&node| {
                            page.doc.nodes[node].parent
                        })
                        .find_map(|node| {
                            computed.values[node]
                                .get(Prop::APP_REGION)
                                .filter(|value| *value != "none")
                        }) == Some("drag")
                    })
                });
                if !dragged {
                    return;
                }
                cx.stop_event();
                if clicks != 2 {
                    view.moving.set(true);
                } else if cfg!(target_os = "macos") {
                    window.control().titlebar_double_click();
                } else {
                    window.control().zoom();
                }
            }
        });
    }
    let parent_display = parent
        .and_then(|parent| styles.values[parent].get(Prop::DISPLAY))
        .unwrap_or("block");
    let axes = [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]].map(|axis| {
        axis.iter()
            .all(|prop| style.get(*prop).is_none_or(|v| v == "auto"))
    });
    let mut anchor = None;
    if position == "absolute" && slot.is_none() && axes.contains(&true) {
        // A grid places a box that it contains by its alignment.
        if matches!(parent_display, "grid" | "inline-grid") {
            if positioner != parent {
                if axes[0] {
                    el = el.left(px(0.));
                }
                if axes[1] {
                    el = el.top(px(0.));
                }
            }
        } else if !matches!(parent_display, "flex" | "inline-flex") {
            let block =
                !display.starts_with("inline") && !style.contains(Prop::INTERNAL_STATIC_INLINE);
            if block
                && axes[0]
                && let Some(parent) = parent
            {
                // The static position is at the start of the parent's content box.
                let (start, rtl) = padding_start(styles, parent, containing[0]);
                reader.log(
                    Query::FlatParentPadding,
                    depth,
                    hashed(Some((start.to_bits(), rtl))),
                );
                el = if rtl {
                    el.right(px(start))
                } else {
                    el.left(px(start))
                };
            }
            anchor = Some((axes, block, [margin[3], margin[0]]));
        }
    }
    if stacking {
        el = el.child(renderer::layers(false));
    }
    el = el.children(bars);
    let sticky = (position == "sticky")
        .then(|| {
            let host = view.host.borrow();
            let scrolls = &host.page_state(view.id)?.geometry.scroll;
            let parent = scroller(doc, styles, ancestors.iter().rev().copied(), scrolls, 0);
            let border = insets(styles, parent, false, containing[0]);
            reader.log(
                Query::Scroller(0),
                ancestors
                    .iter()
                    .rposition(|&ancestor| ancestor == parent)
                    .map_or(1, |index| index + 1),
                hashed((
                    parent,
                    border.map(f32::to_bits),
                    scrolls.contains_key(&parent),
                )),
            );
            let block = ancestors
                .last()
                .map(|&parent| (parent, insets(styles, parent, true, containing[0])));
            reader.log(
                Query::StickyParent,
                depth,
                hashed(block.map(|(parent, insets)| (parent, insets.map(f32::to_bits)))),
            );
            Some(Box::new(super::super::paint::Sticky {
                scroll: scrolls.get(&parent)?.clone(),
                border,
                block,
            }))
        })
        .flatten();
    // Set alignment on replaced elements, controls and ratio-sized blocks to prevent flex-column stretching.
    if hit.is_none()
        && !flex_parent
        && slot.is_none()
        && !display.starts_with("inline")
        && el.style_ref().align_self.is_none()
        && (replaced
            || el.style().aspect_ratio.is_some()
                && matches!(el.style().size.width, renderer::Length::Auto)
                && !matches!(el.style().size.height, renderer::Length::Auto))
    {
        el.style().align_self = Some(if reader.parent(id, Prop::DIRECTION) == Some("rtl") {
            renderer::AlignItems::FlexEnd
        } else {
            renderer::AlignItems::FlexStart
        });
    }
    // An absolute box with both insets of an axis aligns in its inset-modified containing block. Start and end
    // follow the direction of the containing block, and `auto` is `normal`.
    let rtl_block = matches!(position, "absolute" | "fixed")
        && styles.values[positioner.unwrap_or(0)].get(Prop::DIRECTION) == Some("rtl");
    if hit.is_none() && matches!(position, "absolute" | "fixed") {
        let own = style.get(Prop::DIRECTION) == Some("rtl");
        for (prop, insets, inline) in [
            (Prop::JUSTIFY_SELF, [Prop::LEFT, Prop::RIGHT], true),
            (Prop::ALIGN_SELF, [Prop::TOP, Prop::BOTTOM], false),
        ] {
            if insets
                .iter()
                .any(|&inset| style.get(inset).is_none_or(|v| v == "auto"))
            {
                continue;
            }
            let value = style.get(prop).unwrap_or("auto");
            let start = match value.rsplit(' ').next().unwrap_or(value) {
                "center" => Some(renderer::AlignItems::Center),
                "stretch" => Some(renderer::AlignItems::Stretch),
                "start" | "flex-start" | "baseline" => Some(renderer::AlignItems::FlexStart),
                "end" | "flex-end" => Some(renderer::AlignItems::FlexEnd),
                "self-start" | "self-end" | "left" | "right" => {
                    let start = match value {
                        "self-start" => !inline || own == rtl_block,
                        "self-end" => inline && own != rtl_block,
                        "left" => !inline || !rtl_block,
                        _ => inline && rtl_block,
                    };
                    Some(if start {
                        renderer::AlignItems::FlexStart
                    } else {
                        renderer::AlignItems::FlexEnd
                    })
                }
                _ => None,
            };
            if inline {
                el.style().justify_self = start;
            } else {
                el.style().align_self = start;
            }
        }
    }
    let vertical = style
        .get(Prop::WRITING_MODE)
        .filter(|mode| mode.starts_with("vertical") || mode.starts_with("sideways"))
        .filter(|_| id != 0 && !node.tag.starts_with('#') && slot.is_none());
    let mut el = match vertical {
        Some(mode) => {
            let [_, widest, ..] = super::super::layout::grid::intrinsic::intrinsic(
                doc,
                id,
                styles,
                None,
                None,
                text,
                &mut measured.borrow_mut(),
            );
            let inline = style
                .get(Prop::HEIGHT)
                .and_then(|value| units.resolve(value, containing[1]))
                .map(|height| height + extra[1])
                .unwrap_or_else(|| widest.min(units.viewport[1]))
                .max(1.);
            let block = super::super::layout::grid::intrinsic::intrinsic(
                doc,
                id,
                styles,
                Some(inline),
                None,
                text,
                &mut measured.borrow_mut(),
            )[3];
            renderer::node()
                .id(("vertical", id))
                .relative()
                .flex_shrink_0()
                .w(px(block))
                .h(px(inline))
                .child(Turned::node(
                    el.absolute().top_0().left_0().w(px(inline)).into(),
                    mode != "sideways-lr",
                    block,
                    inline,
                ))
        }
        None => el,
    };
    let hooks = DomBox {
        text_clip,
        drawn: node.attrs.contains_key("drawable")
            && style.get(Prop::CANVAS_CONTENT) == Some("drawable")
            && !drawable,
        placed: Default::default(),
        reached_at: Default::default(),
        local: Default::default(),
        offset: Default::default(),
        frame: node.tag == "iframe",
        anchor,
        block: made.absolute.then_some(borders),
        container: positioner.filter(|&block| position == "fixed" || Some(block) != parent_box),
        rtl: rtl_block,
        sized: positioner
            .filter(|&block| block != 0 && Some(block) != parent_box)
            .map(|_| containing),
        style: style.clone(),
        effects: effect_layer,
        composited,
        transformed: info[id].flags & cull::TRANSFORMED != 0,
        perspective: parent_perspective.clone().zip(parent).map(Box::new),
        units,
        revision: info[id].revision,
        sides: [
            padding[1] + padding[3] + borders[1] + borders[3],
            padding[0] + padding[2] + borders[0] + borders[2],
        ],
        margin,
        cell: slot.as_ref().and(parent),
        grid: slot
            .as_ref()
            .filter(|(_, grid)| grid.baseline || grid.first_item.is_some())
            .and_then(|(_, grid)| Some((node.parent?, grid.baseline))),
        shared: (style::role(styles, id) == style::CELL && {
            let table = std::iter::successors(node.parent, |&parent| doc.nodes[parent].parent)
                .find(|&parent| style::table_box(style::role(styles, parent)));
            let collapsed = collapses(doc, styles, id);
            reader.log(
                Query::Collapse,
                table.map_or(0, |table| reader.scope_of(id, table)),
                hashed(collapsed),
            );
            collapsed
        })
        .then(|| borders.map(|width| width / 2.)),
        boxless: matches!(node.tag.as_str(), "option" | "optgroup") && {
            let found = boxless(doc, id);
            reader.log(
                Query::Boxless(id as u32),
                found.map_or(0, |(select, _)| reader.scope_of(id, select)),
                hashed(found.map(|select| select.1)),
            );
            found.is_some_and(|select| select.1)
        },
        text: (display == "inline" && !node.tag.starts_with('#'))
            .then(|| {
                let mut texts = node.children.iter().copied().filter(|&child| {
                    doc.nodes[child].tag == "#text"
                        && !doc.nodes[child].text.trim_ascii().is_empty()
                });
                let first = texts.next()?;
                Some((
                    [first, texts.next_back().unwrap_or(first)].map(|id| id as u32),
                    [padding[0] + borders[0], padding[2] + borders[2]],
                ))
            })
            .flatten(),
        leading: (display == "inline" && !node.tag.starts_with('#')).then(|| {
            let face = text.face(&parsed.face);
            let [ascent, descent, _] = text
                .line_metrics(face, px(units.font.em * pixel_ratio))
                .map(f32::from);
            let line = units.font.lh * pixel_ratio;
            let above = ((line - ascent - descent) / 2.).floor();
            let edges = padding[0] + padding[2] + borders[0] + borders[2];
            [
                units.font.lh + edges,
                above / pixel_ratio,
                (line - above - ascent - descent) / pixel_ratio,
            ]
        }),
        parts: if style::table_box(style::role(styles, id)) {
            let mut rows: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for &cell in &node.children {
                let Some(row) = doc.nodes[cell].parent.filter(|&row| {
                    row != id && (style::ROW..=style::FOOTER).contains(&style::role(styles, row))
                }) else {
                    continue;
                };
                match rows.last_mut() {
                    Some((last, cells)) if *last == row => cells.push(cell),
                    _ => {
                        rows.push((row, vec![cell]));
                        if let Some(group) = doc.nodes[row].parent.filter(|&group| group != id) {
                            match groups.last_mut() {
                                Some((last, members)) if *last == group => members.push(row),
                                _ => groups.push((group, vec![row])),
                            }
                        }
                    }
                }
            }
            let rows = rows.into_iter().map(|(row, cells)| (row, true, cells));
            let rows = rows.chain(groups.into_iter().map(|(group, rows)| (group, false, rows)));
            Some(Rc::new(super::super::paint::Parts::Rows(rows.collect())))
        } else {
            (!cuts.is_empty()).then(|| {
                cut_parts.sort_by_key(|&(_, at, _)| at);
                let at: rustc_hash::FxHashMap<usize, usize> = cuts
                    .iter()
                    .enumerate()
                    .map(|(at, &(cut, ..))| (cut, at))
                    .collect();
                let mut boxes: Vec<_> = cuts.iter().map(|&(cut, ..)| (cut, Vec::new())).collect();
                for (cut, _, piece) in cut_parts {
                    boxes[at[&cut]].1.push(piece);
                }
                Rc::new(super::super::paint::Parts::Cuts {
                    sides: (!multicol)
                        .then_some([padding[3] + borders[3], padding[1] + borders[1]]),
                    boxes,
                })
            })
        },
        id,
        plans,
        ids: ids.clone(),
        view: view.clone(),
        sticky,
        fixed: position == "fixed" && positioner.is_none(),
        inset: [Prop::TOP, Prop::RIGHT, Prop::BOTTOM, Prop::LEFT].map(|prop| {
            let axis = usize::from(matches!(prop, Prop::TOP | Prop::BOTTOM));
            style
                .get(prop)
                .and_then(|v| units.resolve(v, containing[axis]))
        }),
    };
    #[cfg(debug_assertions)]
    reader.extra.set(hashed(format!(
        "{} {:?}",
        reader.extra.get(),
        (
            (
                hooks.anchor,
                hooks.block,
                hooks.container,
                hooks.sized,
                hooks.inset,
                hooks.effects,
                hooks.composited,
                hooks.revision,
                hooks.sides,
                hooks.margin,
                hooks.drawn,
            ),
            (
                hooks.cell,
                hooks.grid,
                &hooks.parts,
                hooks.shared,
                hooks.leading,
                hooks.boxless,
                hooks.text,
                hooks.fixed,
                hooks
                    .sticky
                    .as_ref()
                    .map(|sticky| (sticky.border, sticky.block)),
            ),
        )
    )));
    if hit.is_none() {
        let mut flow = clear_bits(style);
        let placed =
            !flex_parent && slot.is_none() && matches!(position, "static" | "relative" | "sticky");
        let root = node
            .parent
            .is_none_or(|parent| doc.nodes[parent].tag.starts_with('#'));
        match float_side(style) {
            Some(right) if placed => {
                flow |= if right {
                    renderer::Flow::RIGHT
                } else {
                    renderer::Flow::LEFT
                };
            }
            // A block that makes no formatting context of its own: floats around it reach into it.
            None if placed
                && !root
                && matches!(display, "block" | "list-item")
                && el.style_ref().overflow.x == renderer::Overflow::Visible
                && el.style_ref().overflow.y == renderer::Overflow::Visible
                && (el.style_ref().flex_direction == renderer::FlexDirection::Column
                    || el.style_ref().inline == renderer::Inline::Lines)
                && style.get(Prop::CONTAIN).is_none_or(|value| value == "none")
                && style
                    .get(Prop::CONTENT_VISIBILITY)
                    .is_none_or(|value| value == "visible")
                && style
                    .get(Prop::COLUMN_COUNT)
                    .is_none_or(|value| value == "auto")
                && style
                    .get(Prop::COLUMN_WIDTH)
                    .is_none_or(|value| value == "auto")
                && style
                    .get(Prop::ALIGN_CONTENT)
                    .is_none_or(|value| value == "normal") =>
            {
                flow |= renderer::Flow::BLOCK;
            }
            _ => {}
        }
        el.style().flow |= flow;
    }
    let mut element = el.hooks(hooks);
    if let Some(origin) = carry.pseudo_origin {
        element = element.hooks(super::super::paint::ElementOrigin(origin));
    }
    // Like Chrome, positioned boxes and stacking contexts with `z-index: auto` or 0 paint after the in-flow
    // boxes of their stacking context, in tree order.
    if hit.is_none()
        && let Some(order) = order
            .filter(|_| layered)
            .or((positioned || stacking).then_some(0))
        && id != 0
        && !top_layer.contains(&id)
    {
        element = element.z_index(order);
    }
    match (share, &hit) {
        (Some(key), None) => 'store: {
            let own: Option<Box<[Read]>> = reads.borrow()[mark..]
                .iter()
                .filter(|read| usize::from(read.reader) == depth)
                .map(|read| {
                    Some(Read {
                        query: read.query.moved(id as u32, u32::MAX)?,
                        // Unlike kept records, entries hold the box's own reads, so its own scope is 1 and 0
                        // stays the page.
                        scope: if read.scope == 0 {
                            0
                        } else {
                            (depth + 2 - usize::from(read.scope)) as u16
                        },
                        reader: 0,
                        ..*read
                    })
                })
                .collect();
            let Some(own) = own else {
                break 'store;
            };
            let mut shared = view.shared.borrow_mut();
            if shared.len() >= 4096 {
                shared.clear();
            }
            let list = shared.entry(key).or_default();
            if list.len() == 4 {
                list.remove(0);
            }
            list.push(Shared {
                plan: parsed.clone(),
                reads: own,
                style: element.shared_style(),
            });
        }
        #[cfg(debug_assertions)]
        (Some(_), Some(style)) => {
            assert!(
                element.shares(style),
                "box {id} <{}> sets its style after it took a shared one",
                node.tag
            );
            let start = reads.borrow().len();
            let printed = reader.prints.borrow().len();
            let culls = std::cell::Cell::new(0);
            let shadow = Build {
                ancestors: ancestors.clone(),
                perspective: None,
                zone: zone.clone(),
                culls: &culls,
                checking: true,
                extra: Default::default(),
                plan: Default::default(),
                ..*reader
            };
            let fresh = create(&shadow, id, containing, container, None, None);
            reader.prints.borrow_mut().truncate(printed);
            let own = |reads: &[Read]| {
                let mut own: Vec<_> = reads
                    .iter()
                    .filter(|read| usize::from(read.reader) == depth)
                    .map(|read| (read.query, read.value))
                    .collect();
                own.sort_unstable_by_key(|&(query, _)| query);
                own.dedup_by_key(|&mut (query, _)| query);
                own
            };
            let mut log = reads.borrow_mut();
            let (took, built) = (own(&log[mark..start]), own(&log[start..]));
            log.truncate(start);
            assert!(
                took == built,
                "box {id} <{}> reads otherwise than the box whose style it took: {:?} instead of {:?}",
                node.tag,
                took.iter().map(|read| read.0).collect::<Vec<_>>(),
                built.iter().map(|read| read.0).collect::<Vec<_>>()
            );
            let renderer::Child::Node(fresh) = fresh else {
                unreachable!()
            };
            assert!(
                *fresh.style_ref() == **style,
                "box {id} <{}> builds a style that differs from the one it took",
                node.tag
            );
        }
        _ => {}
    }
    element.into()
}
