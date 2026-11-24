use super::super::layout::size::{broken_image, field_size, legend, natural_size, text_field};
use super::super::{
    layout::cull,
    paint::{DomBox, Turned, background::css_background, scrollbar},
};
use super::align::vertical_align;
use super::border::{border_canvas, fieldset_border, inset_shadows, outline, shade_border};
use super::children::children;
use super::columns::columns;
use super::controls::controls;
use super::cx::{Carry, Cx, Frame, above, viewport};
use super::kept::{Query, Read, Replay, Shared, hashed};
use super::margin::{
    body_overflow_moves, clear_bits, definite_line, float_side, in_flow, margins, overscroll,
};
use super::marker::list_marker;
use super::native::{control_layer, native_control};
use super::natural::natural_box;
use super::paragraph::paragraph;
use super::props::props;
use super::pseudos::pseudo_elements;
use super::query::{
    backdrop, box_parent, boxless, collapses, content_editable, inset_axes, insets, padding_start,
    positioner, row_fill, scrolled, scroller, starts_line, text_indent,
};
use super::replaced::replaced;
use super::svg::svg_box;
use super::{Build, element};
use crate::css;
use crate::css::properties::Prop;
use crate::style::{self};
use renderer::{Child, Color, Rect, point, px, relative, size};
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
        siblings: _,
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
        parent,
        block_size,
        opacity,
        effect_layer,
        selection_colors,
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
    let input = (node.tag == "input").then(|| node.input_type());
    let appkit = node.native_control(style);
    if appkit.is_some() {
        super::super::paint::native::register(&view.controls, id);
    }
    let check = input.is_some_and(|kind| {
        kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio")
    });
    let editable = cx.editable;
    let mut field_layer = None;
    if input.is_some() || appkit.is_some() || editable {
        (el, field_layer) = controls(&cx, el, input, appkit);
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
    let _ink = parsed.ink.unwrap_or(Color::BLACK);
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
    let (mut el, native) = native_control(
        &cx,
        el,
        input,
        appkit,
        &mut padding,
        &mut borders,
        &mut border_kinds,
        &mut border_colors,
    );
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
    // The left and right insets of a positioned box take room from its width.
    let inset_x = [Prop::LEFT, Prop::RIGHT].map(|prop| {
        style
            .get(prop)
            .filter(|_| matches!(position, "absolute" | "fixed"))
            .and_then(|v| units.resolve(v, containing[0]))
    });
    let mut fr = Frame {
        padding,
        borders,
        margin,
        extra,
        intrinsic_width: None,
        inset_x,
        natural_ratio,
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
            if !cx.stretched(0)
                && !(matches!(position, "absolute" | "fixed")
                    && [Prop::LEFT, Prop::RIGHT]
                        .iter()
                        .all(|&prop| style.get(prop).is_some_and(|value| value != "auto")))
            {
                el = el.w(px(width + chrome[0]));
            }
            if let Some(min) = fr
                .floor(&cx, 0, width + chrome[0])
                .filter(|_| node.tag != "textarea")
            {
                el = el.min_w(px(min));
            }
        } else if percent {
            el = el.child(super::super::paint::field::natural_width(px(width)));
        }
        if auto[1] && (node.tag != "textarea" || style.get(Prop::FIELD_SIZING) != Some("content")) {
            el = el.min_h(px(height + chrome[1]));
        }
    }
    let _flexbox = cx.flexbox;
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
    if hit.is_some()
        && let Some(value) = style.get(Prop::BOX_SHADOW)
    {
        inner_shadows = css::value::box_shadows(&units, value, true);
    }
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
    el = props(&cx, &mut fr, el, &mut inner_shadows, line_base, in_details);
    if let Some(natural) = natural {
        el = natural_box(&cx, &fr, el, natural, line_base);
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
        el = vertical_align(&cx, &fr, el, parent, replaced, check, input, &native);
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
    if style.get(Prop::OUTLINE_STYLE).is_some() {
        el = outline(&cx, &fr, el, radius);
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
    if native.is_some() || node.tag == "textarea" {
        el = control_layer(&cx, &fr, el, native);
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
    if display == "list-item" {
        el = list_marker(&cx, &fr, el);
    }
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
