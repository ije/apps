use crate::css::{
    properties::BORDERS, properties::MARGIN, properties::PADDING, properties::Prop,
    properties::Style, value::color::color, value::color::paint, value::color::rgba,
    value::tokens::components, value::units::Lengths,
};
use crate::dom::Document;
use crate::style::ComputedStyles;
use crate::style::{
    LineHeight, case, case_ignorable, cursor, font, line_height, pseudo_content, push_full,
    restyle, starts_word, styled_run, text_run, text_transform,
};
use std::rc::Rc;
#[derive(Clone)]
pub(crate) struct InlineText {
    pub text: Rc<str>,
    /// Out-of-flow boxes in the paragraph that its element builds beside it: nodes inside its inline boxes and
    /// nodes at their static place, or `::before` (1) and `::after` (2) of its inline boxes.
    pub lifted: Vec<(usize, u8)>,
    pub runs: Rc<[renderer::TextRun]>,
    pub ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub boxes: Vec<InlineBox>,
    /// The ranges of inline boxes with an outline.
    pub outlines: Vec<usize>,
    pub atoms: Vec<Atom>,
    pub marks: Vec<Mark>,
    pub cursors: Vec<(usize, renderer::Cursor)>,
    pub shadows: Vec<(f32, f32, f32, renderer::Color)>,
    pub first_line: Option<Box<(renderer::TextRun, renderer::TextRun, Option<String>)>>,
    pub line_heights: Vec<(usize, LineHeight)>,
    /// The `::before` and `::after` that are in the text, as runs or atoms.
    pub pseudos: [bool; 2],
    /// A run has another font, size or line height than the block, or emphasis marks, so rows can grow around it.
    pub mixed: bool,
    /// Like Chromium, `break-spaces` and a `::first-line` style turn balance and pretty off.
    pub greedy: bool,
    /// The spliced inline boxes with a part in the text, which the block of the text gives their bounds, and the
    /// first node of the run, which keys the parts.
    pub cuts: Box<[usize]>,
    pub key: usize,
    /// The text has emphasis marks and starts its block, so its first row can reach into the margins before it.
    pub leads: bool,
    fallback: std::cell::OnceCell<bool>,
}

impl InlineText {
    /// Like `mixed`, or a glyph from a fallback font grows a row with a normal line height, as in Chrome.
    pub fn mixed(&self, system: &renderer::TextSystem) -> bool {
        self.mixed
            || *self.fallback.get_or_init(|| {
                let normal = self
                    .line_heights
                    .iter()
                    .any(|(_, kind)| *kind == LineHeight::Normal);
                let mut start = 0;
                normal
                    && self.runs.iter().any(|run| {
                        let part = self.text.get(start..start + run.len).unwrap_or("");
                        start += run.len;
                        if system.segmented(&run.font) {
                            return true;
                        }
                        let face = system.face(&run.font);
                        let ascii = system.ascii_glyphs(face);
                        part.chars().any(|c| match c.is_ascii() {
                            true => ascii >> (c as u32) & 1 == 0 && !c.is_ascii_whitespace(),
                            false => {
                                c != '\u{fffc}' && !c.is_whitespace() && !system.covers(face, c)
                            }
                        })
                    })
            })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Atom {
    pub node: usize,
    pub at: usize,
    pub run: usize,
    /// 1 for `::before` and 2 for `::after` of `node`, else 0.
    pub pseudo: u8,
}

impl Atom {
    /// The key of the atom's measured size: the node, or the node with its pseudo-element in the high bits.
    pub fn key(&self) -> usize {
        atom_key(self.node, self.pseudo)
    }
}

pub(crate) fn atom_key(node: usize, pseudo: u8) -> usize {
    node | usize::from(pseudo) << 30
}

/// The static place of an out-of-flow box in the text.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    pub key: usize,
    /// Past the end of the text when a line break that the text drops comes before it.
    pub at: usize,
    /// The box was a block, so its place is at the start of its line, or of the next line after `opened` edges or text.
    pub block: bool,
    pub opened: bool,
    /// The edges of inline boxes between the last character and the mark, and between it and the next one.
    pub lead: f32,
    pub trail: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct InlineBox {
    pub index: usize,
    pub fill: renderer::Color,
    pub rtl: bool,
    pub margins: [f32; 2],
    pub edges: [f32; 4],
    pub borders: [f32; 4],
    pub border: renderer::Color,
    pub lead: f32,
    pub trail: f32,
    /// The opacity of the spliced boxes around the box and of the box, which its colors take.
    pub fade: f32,
    /// The opacity of a box whose text draws as a group with its background and borders, or 1.
    pub group: f32,
    /// The part of a spliced box goes on past its left or right side.
    pub open: [bool; 2],
}

/// An absolute or fixed box with an inset on both axes, which needs no static place in a line.
pub(crate) fn placed_out_of_flow(style: &Style) -> bool {
    style
        .get(Prop::POSITION)
        .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        && [[Prop::LEFT, Prop::RIGHT], [Prop::TOP, Prop::BOTTOM]]
            .iter()
            .all(|axis| {
                axis.iter()
                    .any(|&prop| style.get(prop).is_some_and(|value| value != "auto"))
            })
}

/// Whether an out-of-flow box takes a static place. Its text has a mark there, which the box follows.
pub(crate) fn static_place(style: &Style) -> bool {
    matches!(style.get(Prop::POSITION), Some("absolute" | "fixed")) && !placed_out_of_flow(style)
}

/// The content of a `::before` or `::after` that takes part in layout. Like Chrome, a plain inline one with an empty
/// string makes no line and leaves the boxes beside it as they are.
pub(crate) fn drawn<'a>(pseudo: &'a Style, parent: &Style) -> Option<&'a str> {
    let content = pseudo.get(Prop::CONTENT)?;
    if matches!(content, "none" | "normal") || pseudo.get(Prop::DISPLAY) == Some("none") {
        return None;
    }
    let same = |prop: Prop| {
        pseudo
            .get(prop)
            .is_none_or(|value| parent.get(prop) == Some(value))
    };
    let unset = |prop: Prop, initial: &str| pseudo.get(prop).is_none_or(|value| value == initial);
    let hollow = matches!(content, "\"\"" | "''")
        && unset(Prop::DISPLAY, "inline")
        && unset(Prop::FLOAT, "none")
        && unset(Prop::VERTICAL_ALIGN, "baseline")
        && unset(Prop::BOX_SHADOW, "none")
        && pseudo
            .get(Prop::OUTLINE_STYLE)
            .is_none_or(|value| value == "none")
        && pseudo
            .get(Prop::POSITION)
            .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
        && [Prop::FONT_FAMILY, Prop::FONT_SIZE, Prop::LINE_HEIGHT]
            .into_iter()
            .all(same)
        && [PADDING, MARGIN]
            .iter()
            .flatten()
            .all(|&prop| unset(prop, "0") || unset(prop, "0px"))
        && BORDERS.iter().all(|[width, kind, _]| {
            unset(*kind, "none") || matches!(pseudo.get(*width), Some("0" | "0px"))
        });
    (!hollow).then_some(content)
}

/// Whether the `::before` and `::after` of the inline box `id` that draw are all out of flow and placed on both axes,
/// so the box stays a part of the text. Adds them to `lifted`.
pub(crate) fn lifts_pseudos(
    styles: &ComputedStyles,
    id: usize,
    lifted: &mut Vec<(usize, u8)>,
) -> bool {
    let Some(pseudos) = styles.generated(id) else {
        return true;
    };
    let drawn = |pseudo: &Style| drawn(pseudo, &styles.values[id]).is_some();
    if !pseudos
        .iter()
        .all(|pseudo| !drawn(pseudo) || placed_out_of_flow(pseudo))
    {
        return false;
    }
    lifted.extend(
        (0..2)
            .filter(|&at| drawn(&pseudos[at]))
            .map(|at| (id, at as u8 + 1)),
    );
    true
}

/// Whether `id` has a `::before` or `::after` that takes part in layout.
fn generates(styles: &ComputedStyles, id: usize) -> bool {
    styles.generated(id).is_some_and(|pseudos| {
        (pseudos.iter()).any(|pseudo| drawn(pseudo, &styles.values[id]).is_some())
    })
}

/// Whether the `::after` of `id` is out of flow at its static place in the last line.
pub(crate) fn static_after(styles: &ComputedStyles, id: usize) -> bool {
    styles.generated(id).is_some_and(|pseudos| {
        let after = &pseudos[1];
        static_place(after)
            && after.contains(Prop::INTERNAL_STATIC_INLINE)
            && after
                .get(Prop::CONTENT)
                .is_some_and(|value| !matches!(value, "none" | "normal"))
            && after.get(Prop::DISPLAY) != Some("none")
    })
}

/// For each parent, the index of its last child that paints in a layer, and the index where its last child search
/// ended. Callers ask for children in order, so the next search starts there.
pub(crate) type Layers = rustc_hash::FxHashMap<usize, (Option<usize>, usize)>;

// Whether a box after `child` in the paragraph of `id` paints in a layer of its own, like a positioned box. It must
// paint above `child`, which the paragraph builds after its text. `child` can be inside inline boxes of the paragraph.
pub(crate) fn layered_after(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
    child: usize,
    layers: &mut Layers,
) -> bool {
    // A `display: contents` box is not in the child list of its parent, but its children are, so the list can
    // hold a lower node of the chain.
    let (mut at, mut listed) = (child, child);
    while at != id
        && let Some(parent) = doc.nodes[at].parent
    {
        let children = &doc.nodes[parent].children;
        let (last, cursor) = layers.entry(parent).or_insert_with(|| {
            let last = children
                .iter()
                .rposition(|&node| layered(doc, styles, node));
            (last, 0)
        });
        let found = |&node: &usize| node == at || node == listed;
        let start = (*cursor).min(children.len());
        if let Some(index) = children[start..]
            .iter()
            .position(found)
            .map(|index| index + start)
            .or_else(|| children[..start].iter().position(found))
        {
            *cursor = index;
            listed = children[index];
            if last.is_some_and(|last| last > index) {
                return true;
            }
        }
        at = parent;
    }
    false
}

// Whether `node` or a box in it paints in a layer of its own.
fn layered(doc: &Document, styles: &ComputedStyles, node: usize) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        let style = &styles.values[node];
        if doc.nodes[node].tag.starts_with('#')
            || style.get(Prop::DISPLAY) == Some("none")
            || placed_out_of_flow(style)
        {
            continue;
        }
        if style
            .get(Prop::POSITION)
            .is_some_and(|value| value != "static")
            || style.get(Prop::OPACITY).is_some_and(|value| value != "1")
            || style.get(Prop::ISOLATION) == Some("isolate")
            || style
                .get(Prop::MIX_BLEND_MODE)
                .is_some_and(|value| value != "normal")
            || [
                Prop::TRANSFORM,
                Prop::OFFSET_PATH,
                Prop::TRANSLATE,
                Prop::ROTATE,
                Prop::SCALE,
                Prop::FILTER,
                Prop::BACKDROP_FILTER,
            ]
            .into_iter()
            .any(|prop| style.get(prop).is_some_and(|value| value != "none"))
        {
            return true;
        }
        pending.extend(&doc.nodes[node].children);
    }
    false
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
        | "input" | "button" | "select" | "textarea" | "progress" | "meter" | "terminal" => {
            return false;
        }
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
        && !generates(styles, id)
        && [PADDING, MARGIN].into_iter().all(|edges| {
            let [_, right, _, left] = units.edges(style, edges, 0.);
            right == 0. && left == 0.
        })
        && BORDERS.iter().enumerate().all(|(side, [width, kind, _])| {
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

pub(crate) fn pop(
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
    if pseudo_blocks(styles, id).is_some_and(|blocks| blocks != [false; 2]) {
        return true;
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
            _ if inline(child)
                && pseudo_blocks(styles, child).is_some_and(|blocks| blocks != [false; 2]) =>
            {
                return true;
            }
            value if value == Some("contents") || inline(child) => stack.push(std::mem::replace(
                &mut children,
                doc.nodes[child].children.iter(),
            )),
            Some(value) if !value.starts_with("inline") => return true,
            _ => {}
        }
    }
}

/// Like CSS 2.1, an inline box that holds a block box splits around it: its content joins the lines of its block
/// container, and the blocks in it sit between those lines. An inline or out-of-flow `::before` or `::after` keeps
/// it a block.
pub(crate) fn spliced(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    holds_block_box(doc, styles, id)
        && pseudo_blocks(styles, id).is_some()
        && line_block(doc, styles, id).is_some_and(|block| {
            let display = styles.values[block].get(Prop::DISPLAY).unwrap_or("block");
            !display.contains("flex")
                && !display.contains("grid")
                && (!display.starts_with("table")
                    || matches!(display, "table-cell" | "table-caption"))
        })
}

/// The opacity of a spliced box, which its parts take, since it has no box of its own.
pub(crate) fn opacity(style: &Style) -> f32 {
    style.get(Prop::OPACITY).map_or(1., |value| {
        match value.strip_suffix('%') {
            Some(percent) => percent.trim().parse::<f32>().map(|value| value / 100.),
            None => value.trim().parse::<f32>(),
        }
        .unwrap_or(1.)
        .clamp(0., 1.)
    })
}

/// Whether the text of a box can draw its opacity: with nothing drawn below or around the text, fading its colors
/// gives the same pixels as a group, so the box stays a part of the lines of its block.
pub(crate) fn fades(style: &Style) -> bool {
    let none =
        |prop: Prop, values: &[&str]| style.get(prop).is_none_or(|value| values.contains(&value));
    none(Prop::BACKGROUND_COLOR, &["transparent", "rgba(0, 0, 0, 0)"])
        && none(Prop::BACKGROUND_IMAGE, &["none"])
        && none(Prop::BOX_SHADOW, &["none"])
        && none(Prop::TEXT_SHADOW, &["none"])
        && none(Prop::OUTLINE_STYLE, &["none"])
        && [
            Prop::BORDER_TOP_STYLE,
            Prop::BORDER_RIGHT_STYLE,
            Prop::BORDER_BOTTOM_STYLE,
            Prop::BORDER_LEFT_STYLE,
        ]
        .into_iter()
        .all(|prop| none(prop, &["none", "hidden"]))
}

/// Whether the lines of a box can draw its opacity: its text takes it, or a layer with its background and borders.
/// Shadows and outlines draw in other passes, so they need a box of its own.
pub(crate) fn groups(style: &Style) -> bool {
    let none = |prop: Prop| style.get(prop).is_none_or(|value| value == "none");
    none(Prop::BOX_SHADOW) && none(Prop::TEXT_SHADOW) && none(Prop::OUTLINE_STYLE)
}

/// Whether `id` has a `::first-letter` or `::first-line` style.
pub(crate) fn first_text(styles: &ComputedStyles, id: usize) -> bool {
    styles
        .extras(id)
        .text
        .as_ref()
        .is_some_and(|pseudos| pseudos[1].is_some() || pseudos[2].is_some())
}

/// Which of `::before` and `::after` of `id` are blocks in the flow, or `None` when one is in the flow of its lines
/// or out of the flow.
pub(crate) fn pseudo_blocks(styles: &ComputedStyles, id: usize) -> Option<[bool; 2]> {
    let Some(pseudos) = styles.generated(id) else {
        return Some([false; 2]);
    };
    let mut blocks = [false; 2];
    for (block, pseudo) in blocks.iter_mut().zip(pseudos.iter()) {
        let display = pseudo.get(Prop::DISPLAY).unwrap_or("inline");
        if drawn(pseudo, &styles.values[id]).is_none() {
            continue;
        }
        if display.starts_with("inline")
            || display == "contents"
            || pseudo.get(Prop::FLOAT).is_some_and(|value| value != "none")
            || pseudo
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        {
            return None;
        }
        *block = true;
    }
    Some(blocks)
}

/// The box that holds the lines of `id`, past `display: contents` and spliced inline boxes.
pub(crate) fn line_block(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<usize> {
    let mut up = doc.nodes[id].parent?;
    while styles.values[up].get(Prop::DISPLAY) == Some("contents") || spliced(doc, styles, up) {
        up = doc.nodes[up].parent?;
    }
    Some(up)
}

/// Whether the lines of its block give `id` a box of its own. A plain inline box whose style they can draw is a
/// part of their text instead.
pub(crate) fn own_box(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    if node.tag.starts_with('#')
        || style
            .get(Prop::DISPLAY)
            .is_some_and(|value| value != "inline")
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
        )
        || {
            let extras = styles.extras(id);
            extras.generated.is_some() || extras.animations.is_some()
        }
    {
        return true;
    }
    std::iter::successors(node.parent, |&up| doc.nodes[up].parent)
        .find(|&up| {
            styles.values[up]
                .get(Prop::DISPLAY)
                .is_some_and(|value| !matches!(value, "inline" | "contents"))
                && !spliced(doc, styles, up)
        })
        .is_none_or(|block| splits(style, &styles.values[block]))
}

// Retain both styles used as pointer keys to prevent address reuse.
pub(crate) fn splits(style: &Rc<Style>, parent: &Rc<Style>) -> bool {
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
                && !(key == Prop::OPACITY && groups(style))
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
                        | Prop::CARET_ANIMATION
                        | Prop::CARET_COLOR
                        | Prop::CARET_SHAPE
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
                        | Prop::TRANSITION
                        | Prop::TRANSITION_PROPERTY
                        | Prop::TRANSITION_DURATION
                        | Prop::TRANSITION_TIMING_FUNCTION
                        | Prop::TRANSITION_DELAY
                        | Prop::TRANSITION_BEHAVIOR
                        | Prop::WILL_CHANGE
                        | Prop::FONT_FEATURE_SETTINGS
                        | Prop::TEXT_DECORATION_SKIP_INK
                        | Prop::HYPHENS
                        | Prop::UNICODE_BIDI
                        | Prop::DIRECTION
                        | Prop::HYPHENATE_CHARACTER
                        | Prop::FONT_VARIANT_NUMERIC
                        | Prop::FONT_VARIANT_LIGATURES
                        | Prop::FONT_VARIANT_CAPS
                        | Prop::FONT_VARIANT_ALTERNATES
                        | Prop::FONT_VARIANT_EAST_ASIAN
                        | Prop::FONT_VARIANT_POSITION
                        | Prop::FONT_SIZE_ADJUST
                        | Prop::FONT_STRETCH
                        | Prop::TEXT_UNDERLINE_POSITION
                        | Prop::TEXT_EMPHASIS_STYLE
                        | Prop::TEXT_EMPHASIS_COLOR
                        | Prop::TEXT_EMPHASIS_POSITION
                        | Prop::FONT_KERNING
                        | Prop::FONT_SYNTHESIS
                        | Prop::FONT_OPTICAL_SIZING
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

/// The siblings before or after `id`, nearest first, and its parent.
pub(crate) fn around(
    doc: &Document,
    id: usize,
    after: bool,
) -> (Box<dyn Iterator<Item = usize> + '_>, Option<usize>) {
    let parent = doc.nodes[id].parent;
    let siblings = parent.map_or(&[][..], |parent| &doc.nodes[parent].children[..]);
    let at = siblings.iter().position(|&sibling| sibling == id);
    let nodes: Box<dyn Iterator<Item = usize>> = match (at, after) {
        (None, _) => Box::new(std::iter::empty()),
        (Some(at), true) => Box::new(siblings[at + 1..].iter().copied()),
        (Some(at), false) => Box::new(siblings[..at].iter().rev().copied()),
    };
    (nodes, parent)
}

/// A parent that keeps its children in the line of its siblings.
pub(crate) fn inline_parent(doc: &Document, styles: &ComputedStyles, id: usize) -> bool {
    !doc.nodes[id].tag.starts_with('#')
        && styles.values[id]
            .get(Prop::DISPLAY)
            .is_none_or(|value| matches!(value, "inline" | "contents"))
        && !holds_block_box(doc, styles, id)
}

/// Whether the inline content of `id` ends with a collapsible space, or with a line break. `None` when it has none.
pub(crate) fn ends_with_space(doc: &Document, styles: &ComputedStyles, id: usize) -> Option<bool> {
    let node = &doc.nodes[id];
    let style = &styles.values[id];
    match node.tag.as_str() {
        "#text" => return node.text.chars().last().map(|c| c.is_ascii_whitespace()),
        "#comment" => return None,
        "br" => return Some(true),
        "img" | "svg" | "canvas" | "video" | "audio" | "iframe" | "embed" | "object" | "input"
        | "button" | "select" | "textarea" | "progress" | "meter" | "terminal" => {
            return Some(false);
        }
        _ => {}
    }
    if style.get(Prop::DISPLAY) == Some("none")
        || style
            .get(Prop::POSITION)
            .is_some_and(|value| matches!(value, "absolute" | "fixed"))
    {
        return None;
    }
    if !inline_parent(doc, styles, id) {
        return Some(
            style
                .get(Prop::DISPLAY)
                .is_some_and(|value| !value.starts_with("inline")),
        );
    }
    (node.children.iter().rev()).find_map(|&child| ends_with_space(doc, styles, child))
}

/// Whether the inline content before `id` in its line ends with a collapsible space, or there is none.
pub(crate) fn space_before(doc: &Document, styles: &ComputedStyles, mut id: usize) -> bool {
    loop {
        let (mut before, parent) = around(doc, id, false);
        if let Some(space) = before.find_map(|sibling| ends_with_space(doc, styles, sibling)) {
            return space;
        }
        match parent.filter(|&parent| inline_parent(doc, styles, parent)) {
            Some(parent) => id = parent,
            None => return true,
        }
    }
}

/// Whether inline content follows `id` in its line.
pub(crate) fn inline_after(doc: &Document, styles: &ComputedStyles, mut id: usize) -> bool {
    loop {
        let (mut after, parent) = around(doc, id, true);
        if let Some(next) = after.find(|&sibling| !makes_no_line(doc, styles, sibling)) {
            let node = &doc.nodes[next];
            return node.tag == "#text"
                || !node.tag.starts_with('#')
                    && node.tag != "br"
                    && styles.values[next]
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| value.starts_with("inline"))
                    && !holds_block_box(doc, styles, next);
        }
        match parent.filter(|&parent| inline_parent(doc, styles, parent)) {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}

/// A part of a spliced inline box in a run: it holds the start or the end of the box, or it is an empty part at the
/// start or the end of the run, which only shows the edges of the box.
pub(crate) const CUT_FIRST: u8 = 1;
pub(crate) const CUT_LAST: u8 = 2;
pub(crate) const CUT_BEFORE: u8 = 4;
pub(crate) const CUT_AFTER: u8 = 8;
// Events of a run: a spliced inline box opens without its children, and closes.
/// What the boxes around a node do to the order of their text.
fn embedding(
    doc: &Document,
    styles: &ComputedStyles,
    node: usize,
) -> Option<Rc<renderer::Embedding>> {
    let mut marks: Vec<(usize, char)> = Vec::new();
    let ups = std::iter::successors(doc.nodes[node].parent, |&up| doc.nodes[up].parent);
    for up in ups {
        let style = &styles.values[up];
        let inline = style
            .get(Prop::DISPLAY)
            .is_none_or(|value| matches!(value, "inline" | "contents"));
        if let Some(value) = style.get(Prop::UNICODE_BIDI) {
            let rtl = style.get(Prop::DIRECTION) == Some("rtl");
            let pick = |ltr, other| if rtl { other } else { ltr };
            let dir = doc.nodes[up].attrs.get("dir");
            let auto = value == "isolate"
                && match dir {
                    Some(dir) => dir.eq_ignore_ascii_case("auto"),
                    None => doc.nodes[up].tag == "bdi",
                };
            // From the innermost mark to the outermost.
            if value.ends_with("override") {
                marks.push((up, pick('\u{202d}', '\u{202e}')));
            }
            match value {
                _ if !inline => {}
                "plaintext" => marks.push((up, '\u{2068}')),
                _ if auto => marks.push((up, '\u{2068}')),
                "embed" => marks.push((up, pick('\u{202a}', '\u{202b}'))),
                "isolate" | "isolate-override" => marks.push((up, pick('\u{2066}', '\u{2067}'))),
                _ => {}
            }
        }
        if !inline {
            break;
        }
    }
    marks.reverse();
    (!marks.is_empty()).then(|| {
        Rc::new(renderer::Embedding {
            marks,
            boxes: Vec::new(),
        })
    })
}

pub(crate) const OPEN: usize = usize::MAX;
pub(crate) const CLOSE: usize = usize::MAX - 1;

/// The text of the inline content of `id`, or of `run`, a part of its children between its blocks. For a text node,
/// `run` holds the text nodes after it that make one flex or grid item with it. `cuts` are the spliced inline boxes
/// with a part in `run`, with their `CUT_*` flags.
pub(crate) fn inline_text(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
    run: &[usize],
    cuts: &[(usize, u8)],
    ends: [bool; 2],
) -> Option<InlineText> {
    let parent = &styles.values[id];
    let own = doc.nodes[id].tag == "#text";
    // An element with no children is a line of its own only through an in-flow inline pseudo-element.
    let pseudo_line = || {
        styles.generated(id).is_some_and(|pseudos| {
            pseudos.iter().any(|pseudo| {
                drawn(pseudo, parent).is_some()
                    && pseudo
                        .get(Prop::DISPLAY)
                        .is_none_or(|value| value.starts_with("inline"))
                    && pseudo.get(Prop::FLOAT).is_none_or(|value| value == "none")
                    && !pseudo
                        .get(Prop::POSITION)
                        .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            })
        })
    };
    if !own && doc.nodes[id].children.is_empty() && !pseudo_line()
        || matches!(
            doc.nodes[id].tag.as_str(),
            "select" | "optgroup" | "progress" | "meter"
        )
        || parent
            .get(Prop::DISPLAY)
            .is_some_and(|value| matches!(value, "grid" | "flex" | "inline-flex" | "inline-grid"))
    {
        return None;
    }
    // The parent box, past `display: contents` elements.
    let item = std::iter::successors(doc.nodes[id].parent, |&up| doc.nodes[up].parent)
        .map(|up| styles.values[up].get(Prop::DISPLAY).unwrap_or("inline"))
        .find(|value| *value != "contents")
        .is_some_and(|value| value.ends_with("flex") || value.ends_with("grid"));
    // A text node or an inline box built alone shares its line with the inline siblings around it.
    let shares = own
        || parent
            .get(Prop::DISPLAY)
            .is_none_or(|value| value == "inline");
    let alone = shares && !item;
    // Like CSS, a space at the end stays when inline content follows in the line, and the next part drops its own.
    let trailing = alone && inline_after(doc, styles, id);
    let mut text = String::new();
    // Two runs fit a small block of the runtime's allocator, four don't.
    let mut runs: Vec<renderer::TextRun> = Vec::with_capacity(2);
    let mut break_spaces = false;
    let mut line_heights = Vec::new();
    let mut ranges = Vec::new();
    let mut boxes: Vec<InlineBox> = Vec::new();
    let mut outlines = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    let atomic_allowed = !own
        && !parent
            .get(Prop::DISPLAY)
            .is_some_and(|value| value.ends_with("flex") || value.ends_with("grid"));
    let mut opening = 0f32;
    let mut leads: Vec<(f32, Option<(usize, f32)>)> = Vec::new();
    // The static places, with the run that comes next and the edges that are open before it.
    let mut marks: Vec<(Mark, usize, f32)> = Vec::new();
    let mark = |key, block, at, runs: &[renderer::TextRun], opening: f32| {
        let trail = runs.last().map_or(0., |run| f32::from(run.inset[1])) + opening;
        let opened = opening > 0.;
        let trailed = Mark {
            key,
            at,
            block,
            opened,
            lead: 0.,
            trail: 0.,
        };
        (trailed, runs.len(), trail)
    };
    let mut led = 0;
    let mut cursors = Vec::new();
    // In-flow inline `::before` and `::after` are a part of the text: plain text as runs, and boxes as atoms.
    // The paragraph of a block marks the static places of out-of-flow boxes. An inline box is too narrow to lay them
    // out in.
    let markers = atomic_allowed && !shares;
    // Out-of-flow boxes in the paragraph that its element builds beside it: nodes, or `::before` (1) and `::after`
    // (2) of inline boxes.
    let mut lifted: Vec<(usize, u8)> = Vec::new();
    let mut layers = Layers::default();
    let parts: [Option<(String, bool)>; 2] = [0, 1].map(|index| {
        let pseudo = &styles.generated(id).filter(|_| atomic_allowed)?[index];
        let content = drawn(pseudo, parent)?;
        let display = pseudo.get(Prop::DISPLAY).unwrap_or("inline");
        if !display.starts_with("inline")
            || pseudo.get(Prop::FLOAT).is_some_and(|value| value != "none")
            || pseudo
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
        {
            return None;
        }
        let units = &styles.lengths[id];
        let boxed = display != "inline"
            || [PADDING, MARGIN]
                .iter()
                .any(|edges| units.edges(pseudo, *edges, 0.) != [0.; 4])
            || units.borders(pseudo, 0.) != [0.; 4]
            || pseudo
                .get(Prop::BACKGROUND_COLOR)
                .and_then(paint)
                .is_some_and(|color| color.a != 0.)
            || pseudo
                .get(Prop::VERTICAL_ALIGN)
                .is_some_and(|value| value != "baseline")
            || pseudo.get(Prop::OPACITY).is_some_and(|value| value != "1")
            || pseudo
                .get(Prop::POSITION)
                .is_some_and(|value| value != "static")
            || [
                Prop::BACKGROUND_IMAGE,
                Prop::BOX_SHADOW,
                Prop::TRANSFORM,
                Prop::OFFSET_PATH,
                Prop::TRANSLATE,
                Prop::ROTATE,
                Prop::SCALE,
                Prop::FILTER,
                Prop::WEBKIT_FILTER,
                Prop::MASK_IMAGE,
                Prop::WEBKIT_MASK_IMAGE,
                Prop::BACKDROP_FILTER,
                Prop::WEBKIT_BACKDROP_FILTER,
            ]
            .into_iter()
            .any(|prop| pseudo.get(prop).is_some_and(|value| value != "none"))
            || pseudo
                .get(Prop::MIX_BLEND_MODE)
                .is_some_and(|value| value != "normal");
        Some((pseudo_content(doc, id, pseudo, content), boxed))
    });
    // A run with spliced boxes holds the pseudo-elements and the first line of the box when it starts or ends it.
    let had = parts.iter().any(Option::is_some) || first_text(styles, id);
    let parts = match !own && !cuts.is_empty() {
        true => {
            let [before, after] = parts;
            [before.filter(|_| ends[0]), after.filter(|_| ends[1])]
        }
        false => parts,
    };
    let mut space = !alone || space_before(doc, styles, id);
    // A pseudo-element is the node `usize::MAX - index` here.
    let mut pending: Vec<_> = if own {
        (run.iter().rev().chain([&id]))
            .map(|&node| (node, None, None))
            .collect()
    } else if !run.is_empty() || !cuts.is_empty() {
        // The pseudo-elements and first line of the box belong to its first and last lines, not to a run.
        if had && cuts.is_empty() {
            return None;
        }
        if cuts.is_empty() {
            run.iter().rev().map(|&child| (child, None, None)).collect()
        } else {
            // The spliced boxes around each node open before it and close after their last node in the run.
            let fill = |node: usize| {
                styles.values[node]
                    .get(Prop::BACKGROUND_COLOR)
                    .and_then(paint)
                    .filter(|color| color.a != 0.)
            };
            let chain = |node: usize| {
                let mut chain: Vec<usize> =
                    std::iter::successors(doc.nodes[node].parent, |&up| doc.nodes[up].parent)
                        .take_while(|&up| up != id)
                        .filter(|up| cuts.iter().any(|(cut, _)| cut == up))
                        .collect();
                chain.reverse();
                chain
            };
            let before = parts[0].as_ref().map(|_| (usize::MAX, None, None));
            let (mut events, mut open) =
                (Vec::new(), Vec::<(usize, Option<renderer::Color>)>::new());
            let mut visit = |chain: &[usize], node: Option<usize>| {
                let kept = open
                    .iter()
                    .zip(chain)
                    .take_while(|((a, _), b)| a == *b)
                    .count();
                while open.len() > kept {
                    let (cut, _) = open.pop().unwrap();
                    events.push((cut, Some(CLOSE), None));
                }
                for &cut in &chain[kept..] {
                    let background = open.last().and_then(|(_, fill)| *fill);
                    events.push((cut, Some(OPEN), background));
                    open.push((cut, fill(cut).or(background)));
                }
                if let Some(node) = node {
                    events.push((node, None, open.last().and_then(|(_, fill)| *fill)));
                }
            };
            let empty = |flag: u8| {
                cuts.iter()
                    .filter(move |(_, flags)| flags & flag != 0)
                    .map(|&(cut, _)| {
                        let mut chain = chain(cut);
                        chain.push(cut);
                        chain
                    })
            };
            for chain in empty(CUT_BEFORE) {
                visit(&chain, None);
            }
            for &node in run {
                visit(&chain(node), Some(node));
            }
            for chain in empty(CUT_AFTER) {
                visit(&chain, None);
            }
            visit(&[], None);
            events.splice(0..0, before);
            events.extend(parts[1].as_ref().map(|_| (usize::MAX - 1, None, None)));
            events.reverse();
            events
        }
    } else {
        (parts[1].is_some().then_some(usize::MAX - 1).into_iter())
            .chain(doc.nodes[id].children.iter().rev().copied())
            .chain(parts[0].is_some().then_some(usize::MAX))
            .map(|child| (child, None, None))
            .collect()
    };
    // The opacity of the inline boxes around a node, and of the node itself when it is a spliced box.
    let fade = |node: usize| {
        std::iter::successors(Some(node).filter(|&node| node < doc.nodes.len()), |&up| {
            doc.nodes[up].parent
        })
        .take_while(|&up| up != id)
        .filter(|&up| {
            up != node
                && !doc.nodes[up].tag.starts_with('#')
                && styles.values[up].get(Prop::DISPLAY) != Some("contents")
                && fades(&styles.values[up])
                || cuts.iter().any(|(cut, _)| *cut == up)
        })
        .map(|up| opacity(&styles.values[up]))
        .product::<f32>()
    };
    // The open parts of spliced boxes: their range and whether they hold the end of the box.
    let mut parted: Vec<(usize, bool)> = Vec::new();
    'nodes: while let Some((child, end, background)) = pending.pop() {
        let part = (end == Some(OPEN))
            .then(|| cuts.iter().find(|(cut, _)| *cut == child))
            .flatten()
            .map(|&(_, flags)| flags);
        let (end, last) = match end {
            Some(CLOSE) => match parted.pop() {
                Some((index, last)) => (Some(index), last),
                None => continue 'nodes,
            },
            Some(OPEN) => (None, true),
            end => (end, true),
        };
        if let Some(index) = end {
            let (_, range): &mut (usize, std::ops::Range<usize>) = &mut ranges[index];
            range.end = text.len();
            let empty = range.start == range.end;
            if let Ok(at) = boxes.binary_search_by_key(&index, |inline| inline.index) {
                let inline = &boxes[at];
                // The end of a right-to-left box is at its left.
                let end = if inline.rtl { 3 } else { 1 };
                let right = inline.edges[end];
                let margin = if last {
                    styles.lengths[child].edges(&styles.values[child], MARGIN, 0.)[end]
                } else {
                    0.
                };
                if empty {
                    opening += right + margin;
                } else if let Some(last) = runs.len().checked_sub(1) {
                    runs[last].inset[1] += renderer::px(right);
                    leads[at].1 = Some((last, f32::from(runs[last].inset[1])));
                    runs[last].inset[1] += renderer::px(margin);
                }
            }
            continue 'nodes;
        }
        let pseudo = parts
            .get(usize::MAX - child)
            .and_then(Option::as_ref)
            .map(|(words, boxed)| (usize::MAX - child, words.as_str(), *boxed));
        // A pseudo-element's text gets the font size of the pseudo-element.
        let (node_text, style, own_style, units, atom): (
            &str,
            &Style,
            Option<&Rc<Style>>,
            Lengths,
            Option<usize>,
        ) = 'source: {
            if let Some((index, words, boxed)) = pseudo {
                let style = &styles.generated(id).unwrap()[index];
                let mut units = styles.lengths[id];
                if let Some(size) = style
                    .get(Prop::FONT_SIZE)
                    .and_then(|value| units.resolve(value, units.font.em))
                {
                    units.font.em = size;
                }
                break 'source (words, style, None, units, boxed.then_some(id));
            }
            let node = &doc.nodes[child];
            let style = &styles.values[child];
            let units = styles.lengths[child];
            if node.tag == "#comment"
                || !node.tag.starts_with('#') && style.get(Prop::DISPLAY) == Some("none")
            {
                continue 'nodes;
            }
            // A float of the block is a box beside the paragraph, not a part of it.
            if atomic_allowed
                && style.get(Prop::FLOAT).is_some_and(|value| value != "none")
                && style
                    .get(Prop::POSITION)
                    .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
            {
                if node.parent == Some(id) || run.contains(&child) {
                    continue 'nodes;
                }
                return None;
            }
            // Like a float, an out-of-flow child placed on both axes is a box beside the paragraph. Others keep
            // the pieces, which give them their static place in the line and their paint order. In the paragraph of
            // a block, such boxes inside its inline boxes are boxes beside it too.
            if atomic_allowed
                && (node.parent == Some(id) || markers)
                && placed_out_of_flow(style)
                && !layered_after(doc, styles, id, child, &mut layers)
            {
                if node.parent != Some(id) {
                    lifted.push((child, 0));
                }
                continue 'nodes;
            }
            if markers && static_place(style) && !layered_after(doc, styles, id, child, &mut layers)
            {
                lifted.push((child, 0));
                marks.push(mark(
                    child,
                    !style.contains(Prop::INTERNAL_STATIC_INLINE),
                    text.len(),
                    &runs,
                    opening,
                ));
                continue 'nodes;
            }
            if node.tag == "br" {
                if text.ends_with(' ')
                    && !style
                        .get(Prop::WHITE_SPACE)
                        .is_some_and(|value| matches!(value, "pre" | "pre-wrap" | "break-spaces"))
                {
                    pop(&mut text, &mut runs, &mut ranges);
                    for (mark, ..) in &mut marks {
                        mark.at = mark.at.min(text.len());
                    }
                }
                ranges.push((child, text.len()..text.len()));
                text.push('\n');
                let mut run = text_run(style, &styles.lengths[child], 1);
                if let Some(clear) = style.get(Prop::CLEAR).filter(|clear| *clear != "none") {
                    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
                    run.clear = match (clear, rtl) {
                        ("left", _) | ("inline-start", false) | ("inline-end", true) => 1,
                        ("both", _) => 3,
                        _ => 2,
                    };
                }
                runs.push(run);
                line_heights.push((text.len(), line_height(style)));
                space = true;
                continue 'nodes;
            }
            let display = style.get(Prop::DISPLAY);
            if atomic_allowed
                && (display.is_some_and(|value| value.starts_with("inline-"))
                    || display.is_none_or(|value| value == "inline")
                        && matches!(
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
                        )
                    // In the lines of a spliced box, an inline box that they can't draw is a box of its own, so
                    // the lines stay one text.
                    || !cuts.is_empty()
                        && part.is_none()
                        && node.tag != "#text"
                        && display.is_none_or(|value| value == "inline")
                        && ({
                            let extras = styles.extras(child);
                            extras.generated.is_some() || extras.animations.is_some()
                        } || splits(style, parent)))
                && style
                    .get(Prop::POSITION)
                    .is_none_or(|value| !matches!(value, "absolute" | "fixed"))
                && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
            {
                break 'source ("", &**style, None, units, Some(child));
            }
            if node.tag != "#text"
                && (style
                    .get(Prop::DISPLAY)
                    .is_some_and(|value| value != "inline")
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
                // The pseudo-elements of a spliced box are blocks beside the text, and the text draws the rest of
                // its style.
                || part.is_none()
                    && (generates(styles, child)
                        && !(markers && lifts_pseudos(styles, child, &mut lifted))
                        || styles.extras(child).animations.is_some()
                        || splits(style, parent))
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
            if node.tag != "#text"
                && style
                    .get(Prop::OUTLINE_STYLE)
                    .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
            {
                outlines.push(index);
            }
            let inherited_background = background;
            let background = style
                .get(Prop::BACKGROUND_COLOR)
                .and_then(paint)
                .filter(|color| color.a != 0.)
                .or(background);
            if node.tag != "#text" {
                let units = &styles.lengths[child];
                let mut padding = units.edges(style, PADDING, 0.);
                let mut margin = units.edges(style, MARGIN, 0.);
                let mut borders = units.borders(style, 0.);
                // A part of a spliced box is open at the sides where the box goes on.
                for (flag, side) in [(CUT_FIRST, 3), (CUT_LAST, 1)] {
                    if part.is_some_and(|flags| flags & flag == 0) {
                        (padding[side], margin[side], borders[side]) = (0., 0., 0.);
                    }
                }
                let fill = (background != inherited_background)
                    .then_some(background)
                    .flatten()
                    .unwrap_or_default();
                let edges: [f32; 4] = std::array::from_fn(|side| padding[side] + borders[side]);
                // The parts of a spliced box draw its shadow.
                if !fill.is_transparent()
                    || edges != [0.; 4]
                    || margin[1] != 0.
                    || margin[3] != 0.
                    || part.is_some()
                        && style
                            .get(Prop::BOX_SHADOW)
                            .is_some_and(|value| value != "none")
                {
                    let rtl = style.get(Prop::DIRECTION) == Some("rtl");
                    let start = if rtl { 1 } else { 3 };
                    opening += margin[start];
                    leads.push((opening, None));
                    opening += edges[start];
                    let faded = fade(child);
                    boxes.push(InlineBox {
                        index,
                        fill: fill.fade(faded),
                        rtl,
                        margins: [margin[3], margin[1]],
                        edges,
                        borders,
                        border: BORDERS
                            .iter()
                            .enumerate()
                            .filter(|(side, _)| borders[*side] > 0.)
                            .find_map(|(_, [_, _, color])| style.get(*color).and_then(paint))
                            .unwrap_or_default()
                            .fade(faded),
                        lead: f32::NAN,
                        trail: f32::NAN,
                        fade: faded,
                        group: match part.is_none() && !fades(style) {
                            true => opacity(style),
                            false => 1.,
                        },
                        open: [CUT_FIRST, CUT_LAST]
                            .map(|flag| part.is_some_and(|flags| flags & flag == 0)),
                    });
                }
            }
            if let Some(flags) = part {
                parted.push((index, flags & CUT_LAST != 0));
                continue 'nodes;
            }
            pending.push((child, Some(index), background));
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|&child| (child, None, background)),
            );
            (node.text.as_str(), &**style, Some(style), units, None)
        };
        if let Some(node) = atom {
            let at = text.len();
            text.push('\u{fffc}');
            let mut run = text_run(parent, &styles.lengths[id], '\u{fffc}'.len_utf8());
            run.underline = None;
            run.strikethrough = None;
            run.overline = None;
            run.color = renderer::Color::TRANSPARENT;
            run.nowrap = style.get(Prop::WHITE_SPACE) == Some("nowrap");
            run.font_size = Some(renderer::px(0.));
            run.inset[0] = renderer::px(opening);
            for (inline, (mark, _)) in boxes[led..].iter_mut().zip(&leads[led..]) {
                inline.lead = opening - mark;
            }
            led = boxes.len();
            opening = 0.;
            atoms.push(Atom {
                node,
                at,
                run: runs.len(),
                pseudo: pseudo.map_or(0, |(index, ..)| index as u8 + 1),
            });
            runs.push(run);
            line_heights.push((text.len(), LineHeight::Fixed(0.)));
            space = false;
            continue;
        }
        if !node_text.is_empty() {
            let start = text.len();
            let mode = style.get(Prop::WHITE_SPACE).unwrap_or("normal");
            break_spaces |= mode == "break-spaces";
            let preserve = matches!(mode, "pre" | "pre-wrap" | "break-spaces");
            let transform = style.get(Prop::TEXT_TRANSFORM).unwrap_or("");
            let mut last = text.chars().rev();
            let (mut prev, mut before) = (last.next().unwrap_or(' '), last.next().unwrap_or(' '));
            text.reserve(node_text.len());
            // A cased letter before, in this text node, for a final sigma.
            let mut cased = false;
            let mut chars = node_text.chars();
            while let Some(c) = chars.next() {
                let after_cased = cased;
                if !case_ignorable(c) {
                    cased = c.is_alphabetic();
                }
                if c.is_ascii_whitespace() && !preserve && !(c == '\n' && mode == "pre-line") {
                    if !space {
                        text.push(' ');
                        (before, prev) = (prev, ' ');
                    }
                    space = true;
                    continue;
                }
                // Like Chrome, uppercase and lowercase take the full case mappings, like `ß` to `SS`.
                let full = match transform {
                    "uppercase" => push_full(&mut text, c, c.to_uppercase()),
                    "lowercase" => push_full(&mut text, c, c.to_lowercase()),
                    _ => None,
                };
                if let Some(last) = full {
                    (before, prev) = (prev, last);
                    space = false;
                    continue;
                }
                // Like Chrome, a lowercase sigma at the end of a word takes its final form.
                let c = if c == 'Σ'
                    && transform == "lowercase"
                    && after_cased
                    && !chars
                        .clone()
                        .find(|&next| !case_ignorable(next))
                        .is_some_and(char::is_alphabetic)
                {
                    'ς'
                } else {
                    case(c, transform, starts_word(before, prev, c))
                };
                (before, prev) = (prev, c);
                text.push(c);
                space = c == '\n';
            }
            if text.len() > start {
                let mut run = match own_style {
                    Some(style) => styled_run(style, &units, text.len() - start),
                    None => text_run(style, &units, text.len() - start),
                };
                let faded = fade(child);
                if faded < 1. {
                    run.color = run.color.fade(faded);
                    run.background_color = run.background_color.map(|color| color.fade(faded));
                    for line in [
                        &mut run.underline,
                        &mut run.strikethrough,
                        &mut run.overline,
                    ] {
                        if let Some(color) = line.as_mut().and_then(|line| line.color.as_mut()) {
                            *color = color.fade(faded);
                        }
                    }
                    run.stroke = run.stroke.map(|(width, color)| (width, color.fade(faded)));
                    if let Some(emphasis) = &mut run.emphasis {
                        let emphasis = std::rc::Rc::make_mut(emphasis);
                        emphasis.color = emphasis.color.fade(faded);
                    }
                }
                let size = units.font.em;
                if (size - styles.lengths[id].font.em).abs() > 0.01 {
                    run.font_size = Some(renderer::px(size));
                }
                run.nowrap = mode == "nowrap";
                // Like Chrome, text that breaks at each letter shows no hyphens.
                if style.get(Prop::HYPHENS) == Some("auto")
                    && style.get(Prop::WORD_BREAK) != Some("break-all")
                {
                    let node = if child < doc.nodes.len() { child } else { id };
                    let language = std::iter::successors(Some(node), |&up| doc.nodes[up].parent)
                        .find_map(|up| doc.nodes[up].attrs.get("lang"));
                    run.hyphenation =
                        crate::style::hyphenation(style, language.map(|value| &**value));
                }
                run.embedding = embedding(
                    doc,
                    styles,
                    if child < doc.nodes.len() { child } else { id },
                );
                run.inset[0] = renderer::px(opening);
                for (inline, (mark, _)) in boxes[led..].iter_mut().zip(&leads[led..]) {
                    inline.lead = opening - mark;
                }
                led = boxes.len();
                opening = 0.;
                runs.push(run);
                line_heights.push((text.len(), line_height(style)));
            }
        }
    }
    if markers && run.is_empty() && (cuts.is_empty() || ends[1]) && static_after(styles, id) {
        marks.push(mark(atom_key(id, 2), false, text.len(), &runs, opening));
    }
    let rtl = parent.get(Prop::DIRECTION) == Some("rtl");
    let plaintext = crate::style::plaintext(doc, styles, id);
    for run in &mut runs {
        run.rtl = rtl;
        run.plaintext = plaintext;
    }
    if text.contains('\r') {
        return None;
    }
    if text.ends_with(' ')
        && !trailing
        && !parent
            .get(Prop::WHITE_SPACE)
            .is_some_and(|value| matches!(value, "pre" | "pre-wrap" | "break-spaces"))
        || text.ends_with('\n')
    {
        let space = text.ends_with(' ');
        pop(&mut text, &mut runs, &mut ranges);
        for (mark, ..) in marks.iter_mut().filter(|_| space) {
            mark.at = mark.at.min(text.len());
        }
    }
    // Like Chrome, the edges of a spliced box next to a block make a line of their own, which an empty object holds.
    if text.is_empty() && !cuts.is_empty() && (run.is_empty() || opening > 0.) {
        text.push('\u{fffc}');
        let mut run = text_run(parent, &styles.lengths[id], text.len());
        run.color = renderer::Color::TRANSPARENT;
        run.font_size = Some(renderer::px(0.));
        run.inset[0] = renderer::px(opening);
        for (inline, (mark, _)) in boxes[led..].iter_mut().zip(&leads[led..]) {
            inline.lead = opening - mark;
        }
        opening = 0.;
        runs.push(run);
        line_heights.push((text.len(), LineHeight::Fixed(0.)));
    }
    if text.is_empty() {
        return None;
    }
    // A line of element boxes only lays out as boxes. Pseudo-element boxes only exist in the text.
    if !atoms.is_empty()
        && marks.is_empty()
        && atoms.iter().all(|atom| atom.pseudo == 0)
        && text.chars().all(|c| c == '\u{fffc}' || c.is_whitespace())
    {
        return None;
    }
    // The edges of empty inline boxes at the end of the text take room after its last run.
    if let Some(last) = runs.last_mut().filter(|_| opening > 0.) {
        last.inset[1] += renderer::px(opening);
    }
    // Before `::first-letter` splits runs, which moves their indices.
    let marks = marks
        .into_iter()
        .map(|(mark, next, trail)| {
            let before = next.checked_sub(1).and_then(|run| runs.get(run));
            let edges = before.map_or(0., |run| f32::from(run.inset[1]))
                + runs.get(next).map_or(0., |run| f32::from(run.inset[0]));
            Mark {
                lead: trail,
                trail: edges - trail,
                ..mark
            }
        })
        .collect();
    for (inline, (mark, closed)) in boxes.iter_mut().zip(&leads) {
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
    if !boxes.is_empty()
        && (rtl
            || boxes.iter().any(|inline| inline.rtl)
            || runs.iter().any(|run| run.embedding.is_some())
            || renderer::reorders(&text))
    {
        let mut start = 0;
        for run in &mut runs {
            let end = start + run.len;
            let around = boxes.iter().filter_map(|inline| {
                let (_, range) = &ranges[inline.index];
                (range.start < end && start < range.end).then(|| renderer::InlineEdges {
                    id: inline.index,
                    rtl: inline.rtl,
                    ends: [start <= range.start, range.end <= end],
                    margins: inline.margins.map(renderer::px),
                    edges: [inline.edges[3], inline.edges[1]].map(renderer::px),
                })
            });
            let around: Vec<_> = around.collect();
            if !around.is_empty() {
                Rc::make_mut(run.embedding.get_or_insert_default()).boxes = around;
            }
            start = end;
        }
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
            .map(rgba);
        run
    };
    let pseudos = styles.extras(id).text.as_ref().filter(|_| {
        !own && ends[0]
            && parent
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
        .and_then(paint)
        .unwrap_or(renderer::Color::BLACK);
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
                        match paint(part) {
                            Some(color) => ink = color,
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
    let (root, height) = (font(parent), line_height(parent));
    let mixed = runs
        .iter()
        .any(|run| run.font_size.is_some() || run.font != root || run.emphasis.is_some())
        || line_heights.iter().any(|(_, kind)| *kind != height);
    let greedy = break_spaces || pseudos.is_some_and(|pseudos| pseudos[2].is_some());
    let leads = ends[0] && runs.iter().any(|run| run.emphasis.is_some());
    Some(InlineText {
        mixed,
        greedy,
        cuts: cuts.iter().map(|&(cut, _)| cut).collect(),
        key: run.first().copied().unwrap_or(id),
        leads,
        fallback: Default::default(),
        text: text.into(),
        runs: runs.into(),
        ranges,
        boxes,
        outlines,
        atoms,
        marks,
        cursors,
        shadows,
        line_heights,
        pseudos: [parts[0].is_some(), parts[1].is_some()],
        lifted,
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

/// The margins that collapse before the first line of `id`, where emphasis marks over that line can reach, as in
/// Chrome. They go through the blocks that start with the line, and end at a block that starts a formatting context
/// around it. Percentages are of the viewport width here.
pub(crate) fn margin_room(doc: &Document, styles: &ComputedStyles, id: usize) -> f32 {
    let flows = |at: usize| {
        let style = &styles.values[at];
        let parent = doc.nodes[at].parent.map(|up| &styles.values[up]);
        matches!(style.get(Prop::DISPLAY), Some("block" | "list-item"))
            && style
                .get(Prop::OVERFLOW_X)
                .is_none_or(|value| value == "visible")
            && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
            && style.get(Prop::CLEAR).is_none_or(|value| value == "none")
            && !style
                .get(Prop::POSITION)
                .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            && parent.is_none_or(|parent| {
                parent
                    .get(Prop::COLUMN_COUNT)
                    .is_none_or(|value| value == "auto")
                    && parent
                        .get(Prop::COLUMN_WIDTH)
                        .is_none_or(|value| value == "auto")
                    && !parent
                        .get(Prop::DISPLAY)
                        .is_some_and(|value| value.contains("flex") || value.contains("grid"))
            })
    };
    let length = |at: usize, prop: Prop| {
        let units = &styles.lengths[at];
        (styles.values[at].get(prop)).map_or(0., |value| {
            units.resolve(value, units.viewport[0]).unwrap_or(0.)
        })
    };
    let mut room = [0f32; 2];
    let mut add = |margin: f32| match margin < 0. {
        true => room[1] = room[1].min(margin),
        false => room[0] = room[0].max(margin),
    };
    let mut at = match doc.nodes[id].tag == "#text" {
        true => doc.nodes[id].parent,
        false => Some(id),
    };
    while let Some(block) = at {
        if !flows(block) {
            return 0.;
        }
        add(length(block, Prop::MARGIN_TOP));
        let up = doc.nodes[block].parent;
        let siblings = up.map_or(&[][..], |up| &doc.nodes[up].children[..]);
        let before = siblings[..siblings.iter().position(|&kid| kid == block).unwrap_or(0)]
            .iter()
            .rev()
            .find(|&&kid| {
                let style = &styles.values[kid];
                !doc.nodes[kid].tag.starts_with('#')
                    && style.get(Prop::DISPLAY) != Some("none")
                    && style.get(Prop::FLOAT).is_none_or(|value| value == "none")
                    && !style
                        .get(Prop::POSITION)
                        .is_some_and(|value| matches!(value, "absolute" | "fixed"))
            });
        if let Some(&before) = before {
            add(length(before, Prop::MARGIN_BOTTOM));
            break;
        }
        at = up.filter(|&up| {
            !doc.nodes[up].tag.starts_with('#')
                && length(up, Prop::PADDING_TOP) == 0.
                && length(up, Prop::BORDER_TOP_WIDTH) == 0.
        });
    }
    (room[0] + room[1]).max(0.)
}

#[cfg(test)]
mod tests;
