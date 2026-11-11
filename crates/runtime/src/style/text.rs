use super::{ComputedStyles, FONT_NAMES, Lengths, Style, color, components};
use crate::dom::Document;
use crate::style::Prop;
use std::rc::Rc;

pub(crate) fn selection_colors(style: &Style, selection: Option<&Rc<Style>>) -> (Option<u32>, u32) {
    let normal = style.get(Prop::COLOR).and_then(color).unwrap_or((0, 255));
    let foreground = selection
        .and_then(|style| style.get(Prop::COLOR))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(normal)
            } else {
                color(value)
            }
        });
    let background = selection
        .and_then(|style| style.get(Prop::BACKGROUND_COLOR))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .unwrap_or(if foreground.is_some() {
            (0, 0)
        } else {
            (0x4488ff, 85)
        });
    let fill = selection
        .and_then(|style| style.get(Prop::WEBKIT_TEXT_FILL_COLOR))
        .and_then(|value| {
            if value.eq_ignore_ascii_case("currentcolor") {
                Some(foreground.unwrap_or(normal))
            } else {
                color(value)
            }
        })
        .or(foreground);
    (
        fill.map(|(color, alpha)| color << 8 | alpha as u32),
        background.0 << 8 | background.1 as u32,
    )
}

#[derive(Clone)]
pub(crate) struct InlineText {
    pub text: Rc<str>,
    pub runs: Vec<renderer::TextRun>,
    pub ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub boxes: Vec<InlineBox>,
    pub atoms: Vec<Atom>,
    pub cursors: Vec<(usize, renderer::Cursor)>,
    pub shadows: Vec<(f32, f32, f32, u32)>,
    pub first_line: Option<Box<(renderer::TextRun, renderer::TextRun, Option<String>)>>,
    pub line_heights: Vec<(usize, LineHeight)>,
}

#[derive(Clone, Copy)]
pub(crate) struct Atom {
    pub node: usize,
    pub at: usize,
    pub run: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct InlineBox {
    pub index: usize,
    pub fill: u32,
    pub edges: [f32; 4],
    pub borders: [f32; 4],
    pub border: u32,
    pub lead: f32,
    pub trail: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum LineHeight {
    Normal,
    Scale(f32),
    Fixed(f32),
}

pub(crate) fn line_height(style: &Style) -> LineHeight {
    match style.get(Prop::LINE_HEIGHT) {
        Some(value) if value.ends_with("px") => value
            .trim_end_matches("px")
            .parse()
            .map_or(LineHeight::Normal, LineHeight::Fixed),
        Some(value) => value.parse().map_or(LineHeight::Normal, LineHeight::Scale),
        None => LineHeight::Normal,
    }
}

pub(crate) fn restyle(
    runs: &[renderer::TextRun],
    range: std::ops::Range<usize>,
    base: &renderer::TextRun,
    pseudo: &renderer::TextRun,
) -> Vec<renderer::TextRun> {
    let mut restyled = Vec::with_capacity(runs.len() + 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let (a, b) = (range.start.clamp(start, end), range.end.clamp(start, end));
        for (from, to, inside) in [(start, a, false), (a, b, true), (b, end, false)] {
            if from >= to {
                continue;
            }
            let mut piece = run.clone();
            piece.len = to - from;
            if inside {
                if piece.color == base.color {
                    piece.color = pseudo.color;
                }
                if piece.font.family == base.font.family {
                    piece.font.family = pseudo.font.family.clone();
                }
                if piece.font.weight == base.font.weight {
                    piece.font.weight = pseudo.font.weight;
                }
                if piece.font.style == base.font.style {
                    piece.font.style = pseudo.font.style;
                }
                if piece.underline == base.underline {
                    piece.underline = pseudo.underline;
                }
                if piece.strikethrough == base.strikethrough {
                    piece.strikethrough = pseudo.strikethrough;
                }
                if piece.font_size == base.font_size {
                    piece.font_size = pseudo.font_size;
                }
                if piece.letter_spacing == base.letter_spacing {
                    piece.letter_spacing = pseudo.letter_spacing;
                }
                if piece.word_spacing == base.word_spacing {
                    piece.word_spacing = pseudo.word_spacing;
                }
                piece.background_color = piece.background_color.or(pseudo.background_color);
            }
            restyled.push(piece);
        }
        start = end;
    }
    restyled
}

pub(crate) fn text_transform(text: &mut String, range: std::ops::Range<usize>, mode: &str) {
    let Some(source) = text.get(range.clone()) else {
        return;
    };
    let mut word = true;
    let mapped: String = source
        .chars()
        .map(|c| {
            let (mut upper, mut lower) = (c.to_uppercase(), c.to_lowercase());
            let mapped = match mode {
                "uppercase" => upper.next().filter(|_| upper.len() == 0),
                "capitalize" if word => upper.next().filter(|_| upper.len() == 0),
                "lowercase" => lower.next().filter(|_| lower.len() == 0),
                _ => None,
            };
            word = !c.is_alphanumeric();
            mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c)
        })
        .collect();
    text.replace_range(range, &mapped);
}

pub(crate) fn cursor(value: &str) -> Option<renderer::Cursor> {
    use renderer::Cursor::*;
    Some(match value {
        "default" => Default,
        "pointer" => Pointer,
        "text" => Text,
        "vertical-text" => VerticalText,
        "crosshair" => Crosshair,
        "grab" | "move" | "all-scroll" => Grab,
        "grabbing" => Grabbing,
        "not-allowed" | "no-drop" => NotAllowed,
        "col-resize" => ColResize,
        "row-resize" => RowResize,
        "ew-resize" => EwResize,
        "ns-resize" => NsResize,
        "e-resize" => EResize,
        "w-resize" => WResize,
        "n-resize" => NResize,
        "s-resize" => SResize,
        "nwse-resize" | "nw-resize" | "se-resize" => NwseResize,
        "nesw-resize" | "ne-resize" | "sw-resize" => NeswResize,
        "alias" => Alias,
        "copy" => Copy,
        "context-menu" => ContextMenu,
        "none" => None,
        _ => return Option::None,
    })
}

// Retain styles used as pointer keys to prevent address reuse.
fn styled_run(style: &Rc<Style>, units: &Lengths, len: usize) -> renderer::TextRun {
    type Runs = rustc_hash::FxHashMap<*const Style, (Rc<Style>, Vec<(Lengths, renderer::TextRun)>)>;
    thread_local!(static RUNS: std::cell::RefCell<Runs> = Default::default());
    let key = style
        .value(Prop::FONT_FAMILY)
        .map_or(std::ptr::null(), |value| value.as_ptr());
    let named = FONT_NAMES.with_borrow(|names| names.1.get(&key).map(|(_, family)| family.clone()));
    let found = named.and_then(|named| {
        RUNS.with_borrow(|runs| {
            runs.get(&Rc::as_ptr(style))?
                .1
                .iter()
                .find(|(kept, run)| kept == units && Rc::ptr_eq(&run.font.family, &named))
                .map(|(_, run)| run.clone())
        })
    });
    let mut run = found.unwrap_or_else(|| {
        let run = text_run(style, units, 0);
        RUNS.with_borrow_mut(|runs| {
            if runs.len() >= 1024 {
                runs.retain(|_, (style, _)| Rc::strong_count(style) > 1);
                if runs.len() >= 1024 {
                    runs.clear();
                }
            }
            let list = &mut runs
                .entry(Rc::as_ptr(style))
                .or_insert_with(|| (style.clone(), Vec::new()))
                .1;
            if list.len() == 4 {
                list.remove(0);
            }
            list.push((*units, run.clone()));
        });
        run
    });
    run.len = len;
    run
}

pub(crate) fn text_run(style: &Style, units: &Lengths, len: usize) -> renderer::TextRun {
    let (ink, alpha) = style.get(Prop::COLOR).and_then(color).unwrap_or((0, 255));
    let ink: renderer::Color = renderer::Color::hex(ink << 8 | alpha as u32);
    let lines = style.get(Prop::TEXT_DECORATION_LINE).unwrap_or("");
    let decoration = style
        .get(Prop::TEXT_DECORATION_COLOR)
        .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
        .and_then(color)
        .map_or(ink, |(ink, alpha)| {
            renderer::Color::hex(ink << 8 | alpha as u32)
        });
    let length = |prop: Prop| {
        style
            .get(prop)
            .and_then(|value| units.resolve(value, units.font.em))
            .map(renderer::px)
    };
    let thickness = length(Prop::TEXT_DECORATION_THICKNESS).unwrap_or(renderer::px(1.));
    let decoration_style = match style.get(Prop::TEXT_DECORATION_STYLE) {
        Some("double") => renderer::DecorationStyle::Double,
        Some("dotted") => renderer::DecorationStyle::Dotted,
        Some("dashed") => renderer::DecorationStyle::Dashed,
        Some("wavy") => renderer::DecorationStyle::Wavy,
        _ => renderer::DecorationStyle::Solid,
    };
    renderer::TextRun {
        len,
        font: font(style),
        color: ink,
        background_color: None,
        underline: lines.contains("underline").then(|| renderer::Decoration {
            thickness,
            color: Some(decoration),
            style: decoration_style,
            offset: length(Prop::TEXT_UNDERLINE_OFFSET),
        }),
        strikethrough: lines
            .contains("line-through")
            .then_some(renderer::Decoration {
                thickness,
                color: Some(decoration),
                style: decoration_style,
                offset: None,
            }),
        letter_spacing: spacing(style, units, Prop::LETTER_SPACING),
        word_spacing: spacing(style, units, Prop::WORD_SPACING),
        font_size: None,
        inset: Default::default(),
        nowrap: false,
    }
}

pub(crate) fn spacing(style: &Style, units: &Lengths, prop: Prop) -> renderer::Pixels {
    renderer::px(
        style
            .get(prop)
            .filter(|value| *value != "normal")
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|value| value.is_finite())
            .unwrap_or(0.),
    )
}

pub(crate) fn makes_no_line(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    match node.tag.as_str() {
        "#comment" => return true,
        "#text" => {
            return node.text.trim_ascii().is_empty()
                && style.get(Prop::WHITE_SPACE).is_none_or(|value| {
                    !matches!(value, "pre" | "pre-wrap" | "pre-line" | "break-spaces")
                });
        }
        "br" | "img" | "svg" | "canvas" | "video" | "audio" | "iframe" | "embed" | "object"
        | "input" | "button" | "select" | "textarea" | "progress" | "meter" => return false,
        _ if node.sf_symbol().is_some() => return false,
        _ => {}
    }
    if style.get(Prop::DISPLAY) == Some("none")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return true;
    }
    let units = &styles.lengths[id];
    style
        .get(Prop::DISPLAY)
        .is_none_or(|value| value == "inline")
        && !styles.pseudos.contains_key(&id)
        && [super::PADDING, super::MARGIN].iter().all(|edges| {
            [1, 3]
                .iter()
                .all(|&side| units.edges(style, *edges, 0.)[side] == 0.)
        })
        && super::BORDERS
            .iter()
            .enumerate()
            .all(|(side, [width, kind, _])| {
                side % 2 == 0
                    || style
                        .get(*kind)
                        .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                    || style
                        .get(*width)
                        .is_some_and(|width| units.resolve(width, 0.) == Some(0.))
            })
        && node
            .children
            .iter()
            .all(|&child| makes_no_line(doc, styles, child))
}

fn pop(
    text: &mut String,
    runs: &mut Vec<renderer::TextRun>,
    ranges: &mut [(usize, std::ops::Range<usize>)],
) {
    text.pop();
    if let Some(run) = runs.last_mut() {
        run.len -= 1;
        if run.len == 0 {
            runs.pop();
        }
    }
    for (_, range) in ranges {
        range.start = range.start.min(text.len());
        range.end = range.end.min(text.len());
    }
}

pub(crate) fn holds_block_box(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let inline = |id: usize| {
        let tag = doc.nodes[id].tag.as_str();
        !tag.starts_with('#')
            && doc.nodes[id].sf_symbol().is_none()
            && styles.values[id]
                .get(Prop::DISPLAY)
                .is_none_or(|value| value == "inline")
            && !matches!(
                tag,
                "svg"
                    | "canvas"
                    | "video"
                    | "audio"
                    | "iframe"
                    | "object"
                    | "embed"
                    | "select"
                    | "textarea"
            )
    };
    if !inline(id) {
        return false;
    }
    let (mut children, mut stack) = (doc.nodes[id].children.iter(), Vec::new());
    loop {
        let Some(&child) = children.next() else {
            match stack.pop() {
                Some(next) => children = next,
                None => return false,
            }
            continue;
        };
        let style = &styles.values[child];
        if doc.nodes[child].tag.starts_with('#')
            || doc.nodes[child].tag == "br"
            || style
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            || style.get(Prop::FLOAT).is_some_and(|value| value != "none")
        {
            continue;
        }
        match style.get(Prop::DISPLAY) {
            Some("none") => {}
            value if value == Some("contents") || inline(child) => stack.push(std::mem::replace(
                &mut children,
                doc.nodes[child].children.iter(),
            )),
            Some(value) if !value.starts_with("inline") => return true,
            _ => {}
        }
    }
}

// Retain both styles used as pointer keys to prevent address reuse.
fn splits(style: &Rc<Style>, parent: &Rc<Style>) -> bool {
    type Pairs = rustc_hash::FxHashMap<(*const Style, *const Style), (Rc<Style>, Rc<Style>, bool)>;
    thread_local!(static PAIRS: std::cell::RefCell<Pairs> = Default::default());
    if Rc::ptr_eq(style, parent) {
        return false;
    }
    let key = (Rc::as_ptr(style), Rc::as_ptr(parent));
    if let Some(split) = PAIRS.with_borrow(|pairs| pairs.get(&key).map(|entry| entry.2)) {
        return split;
    }
    let split = style.get(Prop::WHITE_SPACE) != parent.get(Prop::WHITE_SPACE)
        && style.get(Prop::WHITE_SPACE) != Some("nowrap")
        || [Prop::VISIBILITY, Prop::POINTER_EVENTS, Prop::TEXT_SHADOW]
            .into_iter()
            .any(|prop| style.get(prop) != parent.get(prop))
        || style.iter().any(|(key, value)| {
            key.is_known()
                && parent.value(key) != Some(value)
                && !(key == Prop::POSITION
                    && &**value == "relative"
                    && [
                        Prop::TOP,
                        Prop::RIGHT,
                        Prop::BOTTOM,
                        Prop::LEFT,
                        Prop::Z_INDEX,
                    ]
                    .into_iter()
                    .all(|prop| {
                        style
                            .get(prop)
                            .is_none_or(|value| matches!(value, "auto" | "0" | "0px"))
                    }))
                && !matches!(
                    &**value,
                    "" | "0"
                        | "0px"
                        | "none"
                        | "normal"
                        | "auto"
                        | "inline"
                        | "baseline"
                        | "static"
                        | "visible"
                        | "border-box"
                        | "content-box"
                        | "transparent"
                        | "currentcolor"
                        | "solid"
                        | "0% 0%"
                        | "repeat"
                        | "scroll"
                        | "padding-box"
                        | "auto auto"
                )
                && !matches!(
                    key,
                    Prop::WHITE_SPACE
                        | Prop::PADDING_TOP
                        | Prop::PADDING_RIGHT
                        | Prop::PADDING_BOTTOM
                        | Prop::PADDING_LEFT
                        | Prop::MARGIN_LEFT
                        | Prop::MARGIN_RIGHT
                        | Prop::BORDER_TOP_WIDTH
                        | Prop::BORDER_RIGHT_WIDTH
                        | Prop::BORDER_BOTTOM_WIDTH
                        | Prop::BORDER_LEFT_WIDTH
                        | Prop::BORDER_TOP_STYLE
                        | Prop::BORDER_RIGHT_STYLE
                        | Prop::BORDER_BOTTOM_STYLE
                        | Prop::BORDER_LEFT_STYLE
                        | Prop::COLOR
                        | Prop::FONT_FAMILY
                        | Prop::FONT_WEIGHT
                        | Prop::FONT_STYLE
                        | Prop::TEXT_DECORATION_LINE
                        | Prop::TEXT_DECORATION_COLOR
                        | Prop::TEXT_DECORATION_STYLE
                        | Prop::TEXT_DECORATION_THICKNESS
                        | Prop::TEXT_UNDERLINE_OFFSET
                        | Prop::TEXT_TRANSFORM
                        | Prop::COLOR_SCHEME
                        | Prop::CARET_COLOR
                        | Prop::CURSOR
                        | Prop::FILL
                        | Prop::TEXT_ALIGN
                        | Prop::BACKGROUND_COLOR
                        | Prop::BORDER_TOP_LEFT_RADIUS
                        | Prop::BORDER_TOP_RIGHT_RADIUS
                        | Prop::BORDER_BOTTOM_RIGHT_RADIUS
                        | Prop::BORDER_BOTTOM_LEFT_RADIUS
                        | Prop::BOX_DECORATION_BREAK
                        | Prop::USER_SELECT
                        | Prop::WEBKIT_USER_SELECT
                        | Prop::FONT_SIZE
                        | Prop::BORDER_TOP_COLOR
                        | Prop::BORDER_RIGHT_COLOR
                        | Prop::BORDER_BOTTOM_COLOR
                        | Prop::BORDER_LEFT_COLOR
                        | Prop::LINE_HEIGHT
                        | Prop::LETTER_SPACING
                        | Prop::WORD_SPACING
                        | Prop::TRANSITION_PROPERTY
                        | Prop::TRANSITION_DURATION
                        | Prop::TRANSITION_TIMING_FUNCTION
                        | Prop::TRANSITION_DELAY
                        | Prop::TRANSITION_BEHAVIOR
                        | Prop::WILL_CHANGE
                        | Prop::FONT_FEATURE_SETTINGS
                        | Prop::FONT_VARIATION_SETTINGS
                        | Prop::WEBKIT_FONT_SMOOTHING
                        | Prop::MOZ_OSX_FONT_SMOOTHING
                        | Prop::OUTLINE_COLOR
                        | Prop::OUTLINE_STYLE
                        | Prop::OUTLINE_WIDTH
                        | Prop::OUTLINE_OFFSET
                        | Prop::WIDTH
                        | Prop::HEIGHT
                        | Prop::MIN_WIDTH
                        | Prop::MIN_HEIGHT
                        | Prop::MAX_WIDTH
                        | Prop::MAX_HEIGHT
                        | Prop::ASPECT_RATIO
                        | Prop::MARGIN_TOP
                        | Prop::MARGIN_BOTTOM
                        | Prop::JUSTIFY_SELF
                        | Prop::ALIGN_SELF
                        | Prop::FLEX_GROW
                        | Prop::FLEX_SHRINK
                        | Prop::FLEX_BASIS
                        | Prop::ORDER
                        | Prop::GRID_ROW_START
                        | Prop::GRID_ROW_END
                        | Prop::GRID_COLUMN_START
                        | Prop::GRID_COLUMN_END
                        | Prop::JUSTIFY_ITEMS
                        | Prop::ALIGN_ITEMS
                        | Prop::JUSTIFY_CONTENT
                        | Prop::ALIGN_CONTENT
                        | Prop::FLEX_DIRECTION
                        | Prop::FLEX_WRAP
                        | Prop::ROW_GAP
                        | Prop::COLUMN_GAP
                )
        });
    PAIRS.with_borrow_mut(|pairs| {
        if pairs.len() >= 1024 {
            pairs.retain(|_, (style, parent, _)| {
                Rc::strong_count(style) > 1 && Rc::strong_count(parent) > 1
            });
            if pairs.len() >= 1024 {
                pairs.clear();
            }
        }
        pairs.insert(key, (style.clone(), parent.clone(), split));
    });
    split
}

pub(crate) fn inline_text(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
) -> Option<InlineText> {
    let parent = &styles.values[id];
    let own = doc.nodes[id].tag == "#text";
    if doc.nodes[id].sf_symbol().is_some()
        || !own && doc.nodes[id].children.is_empty()
        || parent
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "grid" | "flex" | "inline-flex" | "inline-grid"))
    {
        return None;
    }
    let siblings = doc.nodes[id]
        .parent
        .map_or(&[][..], |parent| &doc.nodes[parent].children[..]);
    let position = siblings
        .iter()
        .position(|&sibling| sibling == id)
        .unwrap_or(0);
    let item = doc.nodes[id].parent.is_some_and(|parent| {
        styles.values[parent]
            .get(Prop::DISPLAY)
            .is_some_and(|value| value.ends_with("flex") || value.ends_with("grid"))
    });
    let inline_sibling = |sibling: Option<&usize>| {
        own && !item
            && sibling.is_some_and(|&sibling| {
                doc.nodes[sibling].tag == "#text"
                    || !doc.nodes[sibling].tag.starts_with('#')
                        && doc.nodes[sibling].tag != "br"
                        && styles.values[sibling]
                            .get(Prop::DISPLAY)
                            .is_none_or(|value| value.starts_with("inline"))
                        && !holds_block_box(doc, styles, sibling)
            })
    };
    let trailing = inline_sibling(siblings.get(position + 1..).and_then(|rest| {
        rest.iter()
            .find(|&&sibling| !makes_no_line(doc, styles, sibling))
    }));
    let mut text = String::new();
    // Two runs fit a small block of the runtime's allocator, four don't.
    let mut runs: Vec<renderer::TextRun> = Vec::with_capacity(2);
    let mut line_heights = Vec::new();
    let mut ranges = Vec::new();
    let mut boxes: Vec<InlineBox> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    let atomic_allowed = !own
        && !parent
            .get(Prop::DISPLAY)
            .is_some_and(|value| value.ends_with("flex") || value.ends_with("grid"));
    let mut opening = 0f32;
    let mut marks: Vec<(f32, Option<(usize, f32)>)> = Vec::new();
    let mut cursors = Vec::new();
    let [before, after] = [0, 1].map(|index| {
        !own && styles.pseudos.get(&id).is_some_and(|pseudos| {
            let pseudo = &pseudos[index];
            pseudo
                .get(Prop::CONTENT)
                .is_some_and(|value| !matches!(value, "none" | "normal"))
                && pseudo
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| value.starts_with("inline"))
                && !pseudo
                    .get(Prop::POSITION)
                    .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        })
    });
    let trailing = trailing || after;
    let mut space = !before
        && !inline_sibling(siblings.get(..position).and_then(|rest| {
            rest.iter()
                .rev()
                .find(|&&sibling| !makes_no_line(doc, styles, sibling))
        }));
    let mut pending: Vec<_> = if own {
        vec![(id, None, None)]
    } else {
        doc.nodes[id]
            .children
            .iter()
            .rev()
            .map(|&child| (child, None, None))
            .collect()
    };
    while let Some((child, end, background)) = pending.pop() {
        if let Some(index) = end {
            let (_, range): &mut (usize, std::ops::Range<usize>) = &mut ranges[index];
            range.end = text.len();
            let empty = range.start == range.end;
            if let Some(at) = boxes.iter().position(|inline| inline.index == index) {
                let inline = &boxes[at];
                let right = inline.edges[1];
                let margin =
                    styles.lengths[child].edges(&styles.values[child], super::MARGIN, 0.)[1];
                if empty {
                    opening += right + margin;
                } else if let Some(last) = runs.len().checked_sub(1) {
                    runs[last].inset[1] += renderer::px(right);
                    marks[at].1 = Some((last, f32::from(runs[last].inset[1])));
                    runs[last].inset[1] += renderer::px(margin);
                }
            }
            continue;
        }
        let node = &doc.nodes[child];
        let style = &styles.values[child];
        if node.tag == "#comment"
            || !node.tag.starts_with('#') && style.get(Prop::DISPLAY) == Some("none")
        {
            continue;
        }
        if node.tag == "br" {
            if text.ends_with(' ')
                && !style
                    .get(Prop::WHITE_SPACE)
                    .is_some_and(|value| matches!(value, "pre" | "pre-wrap" | "break-spaces"))
            {
                pop(&mut text, &mut runs, &mut ranges);
            }
            ranges.push((child, text.len()..text.len()));
            text.push('\n');
            runs.push(text_run(style, &styles.lengths[child], 1));
            line_heights.push((text.len(), line_height(style)));
            space = true;
            continue;
        }
        let display = style.get(Prop::DISPLAY);
        if atomic_allowed
            && (display.is_some_and(|value| value.starts_with("inline-"))
                || display.is_none_or(|value| value == "inline")
                    && (node.sf_symbol().is_some()
                        || matches!(
                            node.tag.as_str(),
                            "img"
                                | "svg"
                                | "canvas"
                                | "video"
                                | "audio"
                                | "iframe"
                                | "object"
                                | "embed"
                                | "input"
                                | "button"
                                | "select"
                                | "textarea"
                                | "progress"
                                | "meter"
                        )))
            && style
                .get(Prop::POSITION)
                .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
            && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
        {
            let at = text.len();
            text.push('\u{fffc}');
            let mut run = text_run(parent, &styles.lengths[id], '\u{fffc}'.len_utf8());
            run.underline = None;
            run.strikethrough = None;
            run.color = renderer::Color::TRANSPARENT;
            run.nowrap = style.get(Prop::WHITE_SPACE) == Some("nowrap");
            run.font_size = Some(renderer::px(0.01));
            run.inset[0] = renderer::px(opening);
            for (inline, (mark, _)) in boxes.iter_mut().zip(&marks) {
                if inline.lead.is_nan() {
                    inline.lead = opening - mark;
                }
            }
            opening = 0.;
            atoms.push(Atom {
                node: child,
                at,
                run: runs.len(),
            });
            runs.push(run);
            line_heights.push((text.len(), LineHeight::Fixed(0.)));
            space = false;
            continue;
        }
        if node.tag != "#text"
            && (style
                .get(Prop::DISPLAY)
                .is_some_and(|value| value != "inline")
                || node.sf_symbol().is_some()
                || matches!(
                    node.tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "input"
                        | "button"
                        | "select"
                        | "textarea"
                        | "iframe"
                        | "video"
                        | "audio"
                        | "object"
                        | "embed"
                        | "progress"
                        | "meter"
                ))
            || styles.pseudos.contains_key(&child)
            || styles.animations.contains_key(&child)
            || splits(style, parent)
        {
            return None;
        }
        let index = ranges.len();
        ranges.push((child, text.len()..text.len()));
        if node.tag != "#text"
            && let Some(style) = style
                .get(Prop::CURSOR)
                .filter(|value| Some(*value) != parent.get(Prop::CURSOR))
                .and_then(cursor)
        {
            cursors.push((index, style));
        }
        let inherited_background = background;
        let background = style
            .get(Prop::BACKGROUND_COLOR)
            .and_then(color::color)
            .filter(|(_, alpha)| *alpha != 0)
            .or(background);
        if node.tag != "#text" {
            let units = &styles.lengths[child];
            let padding = units.edges(style, super::PADDING, 0.);
            let margin = units.edges(style, super::MARGIN, 0.);
            let borders: [f32; 4] = std::array::from_fn(|side| {
                let [width, kind, _] = super::BORDERS[side];
                if style
                    .get(kind)
                    .is_none_or(|kind| matches!(kind, "none" | "hidden"))
                {
                    return 0.;
                }
                style
                    .get(width)
                    .map_or(Some(3.), |width| units.resolve(width, 0.))
                    .unwrap_or(0.)
                    .max(0.)
            });
            let fill = (background != inherited_background)
                .then_some(background)
                .flatten()
                .map_or(0, |(color, alpha)| color << 8 | alpha as u32);
            let edges: [f32; 4] = std::array::from_fn(|side| padding[side] + borders[side]);
            if fill != 0 || edges != [0.; 4] || margin[1] != 0. || margin[3] != 0. {
                opening += margin[3];
                marks.push((opening, None));
                opening += edges[3];
                boxes.push(InlineBox {
                    index,
                    fill,
                    edges,
                    borders,
                    border: super::BORDERS
                        .iter()
                        .enumerate()
                        .filter(|(side, _)| borders[*side] > 0.)
                        .find_map(|(_, [_, _, color])| style.get(*color).and_then(self::color))
                        .map_or(0, |(color, alpha)| color << 8 | alpha as u32),
                    lead: f32::NAN,
                    trail: f32::NAN,
                });
            }
        }
        pending.push((child, Some(index), background));
        pending.extend(
            node.children
                .iter()
                .rev()
                .map(|&child| (child, None, background)),
        );
        if !node.text.is_empty() {
            let start = text.len();
            let mode = style.get(Prop::WHITE_SPACE).unwrap_or("normal");
            let preserve = matches!(mode, "pre" | "pre-wrap" | "break-spaces");
            let transform = style.get(Prop::TEXT_TRANSFORM);
            let mut word = true;
            text.reserve(node.text.len());
            for c in node.text.chars() {
                if c.is_ascii_whitespace() && !preserve && !(c == '\n' && mode == "pre-line") {
                    if !space {
                        text.push(' ');
                    }
                    space = true;
                    word = true;
                    continue;
                }
                let mapped = match transform {
                    Some("uppercase") => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("capitalize") if word => {
                        let mut mapped = c.to_uppercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    Some("lowercase") => {
                        let mut mapped = c.to_lowercase();
                        mapped.next().filter(|_| mapped.next().is_none())
                    }
                    _ => None,
                };
                let c = mapped.filter(|m| m.len_utf8() == c.len_utf8()).unwrap_or(c);
                text.push(c);
                space = c == '\n';
                word = !c.is_alphanumeric();
            }
            if text.len() > start {
                let mut run = styled_run(style, &styles.lengths[child], text.len() - start);
                let size = styles.lengths[child].font.em;
                if (size - styles.lengths[id].font.em).abs() > 0.01 {
                    run.font_size = Some(renderer::px(size));
                }
                run.nowrap = mode == "nowrap";
                run.inset[0] = renderer::px(opening);
                for (inline, (mark, _)) in boxes.iter_mut().zip(&marks) {
                    if inline.lead.is_nan() {
                        inline.lead = opening - mark;
                    }
                }
                opening = 0.;
                runs.push(run);
                line_heights.push((text.len(), line_height(style)));
            }
        }
    }
    if text.contains(['\t', '\r']) {
        return None;
    }
    if text.ends_with(' ')
        && !trailing
        && !parent
            .get(Prop::WHITE_SPACE)
            .is_some_and(|value| matches!(value, "pre" | "pre-wrap" | "break-spaces"))
        || text.ends_with('\n')
    {
        pop(&mut text, &mut runs, &mut ranges);
    }
    if text.is_empty() {
        return None;
    }
    if !atoms.is_empty() && text.chars().all(|c| c == '\u{fffc}' || c.is_whitespace()) {
        return None;
    }
    let pseudo_run = |style: &Style| {
        let units = &styles.lengths[id];
        let mut run = text_run(style, units, 0);
        run.font_size = style
            .get(Prop::FONT_SIZE)
            .and_then(|value| units.resolve(value, units.font.em))
            .filter(|size| (size - units.font.em).abs() > 0.01)
            .map(renderer::px);
        run.background_color = style
            .get(Prop::BACKGROUND_COLOR)
            .and_then(color)
            .filter(|(_, alpha)| *alpha != 0)
            .map(|(ink, alpha)| renderer::Color::hex(ink << 8 | alpha as u32));
        run
    };
    let pseudos = styles.text_pseudos.get(&id).filter(|_| {
        !own && parent
            .get(Prop::DISPLAY)
            .is_some_and(|value| value != "inline")
    });
    if let Some(letter) = pseudos.and_then(|pseudos| pseudos[1].as_ref()) {
        let start = text.len() - text.trim_start().len();
        let (mut end, mut found) = (start, false);
        for (index, c) in text[start..].char_indices() {
            if c.is_whitespace() || found && c.is_alphanumeric() {
                break;
            }
            found |= c.is_alphanumeric();
            end = start + index + c.len_utf8();
        }
        if found {
            if let Some(mode) = letter
                .get(Prop::TEXT_TRANSFORM)
                .filter(|mode| Some(*mode) != parent.get(Prop::TEXT_TRANSFORM))
            {
                text_transform(&mut text, start..end, mode);
            }
            runs = restyle(
                &runs,
                start..end,
                &text_run(parent, &styles.lengths[id], 0),
                &pseudo_run(letter),
            );
        }
    }
    let ink = parent
        .get(Prop::COLOR)
        .and_then(color)
        .map_or(0xff, |(ink, alpha)| ink << 8 | alpha as u32);
    let shadows = parent
        .get(Prop::TEXT_SHADOW)
        .filter(|value| *value != "none")
        .map_or_else(Vec::new, |value| {
            components(value, ',')
                .into_iter()
                .rev()
                .filter_map(|shadow| {
                    let (mut ink, mut lengths) = (ink, Vec::new());
                    for part in components(shadow, ' ') {
                        match color(part) {
                            Some((color, alpha)) => ink = color << 8 | alpha as u32,
                            None if part.eq_ignore_ascii_case("currentcolor") => {}
                            None => lengths.push(styles.lengths[id].resolve(part, 0.)?),
                        }
                    }
                    match lengths[..] {
                        [x, y] => Some((x, y, 0., ink)),
                        [x, y, blur] => Some((x, y, blur.max(0.), ink)),
                        _ => None,
                    }
                })
                .collect()
        });
    for (inline, (mark, closed)) in boxes.iter_mut().zip(&marks) {
        if inline.lead.is_nan() {
            inline.lead = opening - mark;
        }
        if let Some((run, at)) = closed {
            inline.trail = runs
                .get(*run)
                .map_or(0., |run| f32::from(run.inset[1]) - at)
                + runs.get(run + 1).map_or(0., |run| f32::from(run.inset[0]));
        }
    }
    Some(InlineText {
        text: text.into(),
        runs,
        ranges,
        boxes,
        atoms,
        cursors,
        shadows,
        line_heights,
        first_line: pseudos.and_then(|pseudos| pseudos[2].as_ref()).map(|line| {
            Box::new((
                text_run(parent, &styles.lengths[id], 0),
                pseudo_run(line),
                line.get(Prop::TEXT_TRANSFORM)
                    .filter(|mode| Some(*mode) != parent.get(Prop::TEXT_TRANSFORM))
                    .map(str::to_owned),
            ))
        }),
    })
}

pub(crate) fn word_break(style: &Style) -> renderer::WordBreak {
    match (style.get(Prop::WORD_BREAK), style.get(Prop::OVERFLOW_WRAP)) {
        (Some("break-all"), _) => renderer::WordBreak::BreakAll,
        (Some("break-word"), _) | (_, Some("anywhere")) => renderer::WordBreak::Anywhere,
        (_, Some("break-word")) => renderer::WordBreak::BreakWord,
        _ => renderer::WordBreak::Normal,
    }
}

pub(crate) fn font(style: &Style) -> renderer::Font {
    let value = style.value(Prop::FONT_FAMILY);
    let key = value.map_or(std::ptr::null(), |value| value.as_ptr());
    let family = FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some((_, family)) = names.1.get(&key) {
            return family.clone();
        }
        let families = value.map_or("system-ui", |value| &**value);
        let family: renderer::SharedString = components(families, ',')
            .into_iter()
            .find_map(|family| {
                let family = family.trim().trim_matches(['\'', '"']);
                let lower = family.to_ascii_lowercase();
                let generic: &[&str] = match lower.as_str() {
                    "ui-monospace" | "ui-serif" | "ui-sans-serif" | "ui-rounded" => return None,
                    "system-ui" | "-apple-system" | "blinkmacsystemfont" => &[".SystemUIFont"],
                    "sans-serif" => &[
                        "Helvetica",
                        "Arial",
                        "DejaVu Sans",
                        "Liberation Sans",
                        ".SystemUIFont",
                    ],
                    "monospace" => &[
                        "Menlo",
                        "SF Mono",
                        "Consolas",
                        "DejaVu Sans Mono",
                        "Liberation Mono",
                        "Courier New",
                    ],
                    "serif" => &[
                        "Times",
                        "Times New Roman",
                        "DejaVu Serif",
                        "Liberation Serif",
                    ],
                    "cursive" => &["Apple Chancery", "Comic Sans MS"],
                    "fantasy" => &["Papyrus", "Impact"],
                    _ => {
                        return if names.0.is_empty() {
                            Some(family.to_owned())
                        } else {
                            names.0.get(&lower).cloned()
                        };
                    }
                };
                generic
                    .iter()
                    .find(|name| {
                        name.starts_with('.') || names.0.contains_key(&name.to_ascii_lowercase())
                    })
                    .or(generic.first().filter(|_| names.0.is_empty()))
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| ".SystemUIFont".into())
            .into();
        if !names.0.is_empty() {
            if names.1.len() > 4096 {
                names.1.clear();
            }
            names.1.insert(key, (value.cloned(), family.clone()));
        }
        family
    });
    let mut font = renderer::Font::new(family);
    let weight = match style.get(Prop::FONT_WEIGHT) {
        Some("bold") => 700.,
        Some(value) => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(400.),
        None => 400.,
    };
    font.weight = weight.clamp(1., 1000.);
    font.style = match style.get(Prop::FONT_STYLE) {
        Some("italic") => renderer::FontStyle::Italic,
        Some(value) if value.starts_with("oblique") => renderer::FontStyle::Oblique,
        _ => renderer::FontStyle::Normal,
    };
    let mut features: Vec<(String, u32)> = Vec::new();
    if style.get(Prop::FONT_KERNING) == Some("none") {
        features.push(("kern".into(), 0));
    }
    let keywords = [Prop::FONT_VARIANT_LIGATURES, Prop::FONT_VARIANT_NUMERIC]
        .into_iter()
        .filter_map(|prop| style.get(prop))
        .flat_map(str::split_ascii_whitespace);
    for keyword in keywords {
        let tags: &[(&str, u32)] = match keyword {
            "none" | "no-common-ligatures" => &[("liga", 0), ("clig", 0)],
            "no-contextual" => &[("calt", 0)],
            "discretionary-ligatures" => &[("dlig", 1)],
            "lining-nums" => &[("lnum", 1)],
            "oldstyle-nums" => &[("onum", 1)],
            "proportional-nums" => &[("pnum", 1)],
            "tabular-nums" => &[("tnum", 1)],
            "diagonal-fractions" => &[("frac", 1)],
            "stacked-fractions" => &[("afrc", 1)],
            "ordinal" => &[("ordn", 1)],
            "slashed-zero" => &[("zero", 1)],
            _ => &[],
        };
        features.extend(tags.iter().map(|&(tag, value)| (tag.to_owned(), value)));
    }
    for setting in style
        .get(Prop::FONT_FEATURE_SETTINGS)
        .filter(|value| *value != "normal")
        .map(|value| components(value, ','))
        .unwrap_or_default()
    {
        let mut parts = setting.split_ascii_whitespace();
        let Some(tag) = parts
            .next()
            .map(|tag| tag.trim_matches(['"', '\'']))
            .filter(|tag| tag.len() == 4)
        else {
            continue;
        };
        let value = match parts.next() {
            None | Some("on") => 1,
            Some("off") => 0,
            Some(value) => value.parse().unwrap_or(1),
        };
        features.push((tag.to_owned(), value));
    }
    if !features.is_empty() {
        font.features = features.into();
    }
    font
}
