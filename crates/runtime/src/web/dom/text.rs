use super::display;
use crate::css::properties::Prop;
use crate::dom::Document;
use crate::render::PageState;
use crate::style;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default)]
struct Caret {
    text: Vec<u16>,
    runs: Vec<i64>,
    found: i64,
    waiting: bool,
    edge: bool,
    space: Option<(i64, i64)>,
    // Editable content was skipped before the first character, or after the last.
    hidden: [bool; 2],
}

impl Caret {
    fn push(&mut self, units: &[u16], node: i64, offset: i64) {
        let index = self.text.len() as i64;
        if !matches!(self.runs[..], [.., start, last, from] if last == node && offset >= 0 && from + index - start == offset)
        {
            self.runs.extend([index, node, offset]);
        }
        self.text.extend_from_slice(units);
    }

    fn emit(&mut self, units: &[u16], node: i64, offset: i64) {
        let after = self.text.last() == Some(&10);
        if std::mem::take(&mut self.edge) {
            if !self.text.is_empty() && !after {
                self.push(&[10], -1, -1);
            }
        } else if let Some((at, from)) = self.space
            && units != [10]
            && !self.text.is_empty()
            && !after
        {
            self.push(&[32], at, from);
        }
        self.space = None;
        self.hidden[1] = false;
        self.reach_all();
        self.push(units, node, offset);
    }

    fn reach_all(&mut self) {
        if std::mem::take(&mut self.waiting) {
            self.found = self.text.len() as i64;
        }
    }

    fn reach(&mut self) {
        if self.edge || self.space.is_some() {
            self.waiting = true;
        } else {
            self.found = self.text.len() as i64;
        }
    }

    // Whether the walk passed the point, so the text after it counts.
    fn after_point(&self) -> bool {
        self.found >= 0 || self.waiting
    }

    // Collapsible white space: one space at the first such character, if text follows.
    fn collapse(&mut self, node: i64, offset: i64) {
        self.space.get_or_insert((node, offset));
    }

    // Editable content that the walk skips, before the text or after it.
    fn skip_editable(&mut self) {
        self.hidden = [self.text.is_empty() || self.hidden[0], true];
    }

    // The JSON of `caret_text`, with whether the text reaches the start and the end of the root.
    fn finish(mut self, [start, end]: [bool; 2]) -> Value {
        self.reach_all();
        json!([
            String::from_utf16_lossy(&self.text),
            self.runs,
            self.found,
            start,
            end,
            self.hidden
        ])
    }

    // A block edge drops a pending space, and the points after that space stay before the line break.
    fn block_edge(&mut self) {
        if !self.edge {
            self.reach_all();
        }
        self.space = None;
        self.edge = true;
    }
}

// The text that `Selection.modify()` moves through: collapsed white space, one line break for each block edge
// and `<br>`, and U+FFFC for each replaced element. Runs of `[index, node, offset]` map it back to the DOM: a
// text node and the offset of the index, an element (offset -1) or a block edge (node -1). Only `window`
// siblings on each side of the path to the point, and 512 times that many characters of each text node, are
// read, so the cost does not grow with the document. Returns the text, the runs, the index of the point (-1
// when it is not in the text), whether the text reaches the start and the end of the root, and whether
// editable content comes before its first and after its last character.
fn caret_text(
    doc: &Document,
    computed: &style::ComputedStyles,
    root: usize,
    editable: bool,
    point: Option<(usize, usize)>,
    window: usize,
) -> Value {
    enum Step {
        Open(usize),
        Close,
        Point,
    }
    let mut limits = Vec::new();
    if let Some((node, offset)) = point {
        let (mut at, mut center) = (node, offset);
        if doc.nodes[node].tag == "#text" {
            center = usize::MAX;
        }
        loop {
            let children = &doc.nodes[at].children;
            if center != usize::MAX {
                limits.push((
                    at,
                    center.saturating_sub(window),
                    center.saturating_add(window).min(children.len()),
                ));
            }
            let Some(parent) = doc.nodes[at].parent.filter(|_| at != root) else {
                break;
            };
            center = doc.nodes[parent]
                .children
                .iter()
                .position(|&child| child == at)
                .unwrap_or(0);
            at = parent;
        }
        if at != root {
            limits.clear();
        }
    }
    let mut caret = Caret {
        found: -1,
        ..Default::default()
    };
    let (mut start, mut end) = (true, true);
    let mut steps = vec![Step::Open(root)];
    while let Some(step) = steps.pop() {
        let node = match step {
            Step::Point => {
                caret.reach();
                continue;
            }
            Step::Close => {
                caret.block_edge();
                continue;
            }
            Step::Open(node) => node,
        };
        let current = &doc.nodes[node];
        let style = current
            .parent
            .filter(|_| current.tag == "#text")
            .map_or(&computed.values[node], |parent| &computed.values[parent]);
        if current.tag == "#text" {
            let white_space = style.get(Prop::WHITE_SPACE).unwrap_or("normal");
            let keep = matches!(white_space, "pre" | "pre-wrap" | "break-spaces");
            let hidden = style
                .get(Prop::VISIBILITY)
                .is_some_and(|value| matches!(value, "hidden" | "collapse"));
            let budget = window.saturating_mul(512);
            let ascii = current.text.is_ascii();
            let (lo, hi, length) = if current.text.len() <= budget {
                (0, usize::MAX, 0)
            } else {
                let length = if ascii {
                    current.text.len()
                } else {
                    current.text.encode_utf16().count()
                };
                let (lo, hi) = match point {
                    Some((at, offset)) if at == node => {
                        (offset.saturating_sub(budget), offset.saturating_add(budget))
                    }
                    _ if caret.after_point() => (0, budget),
                    _ => (length.saturating_sub(budget), usize::MAX),
                };
                (lo, hi, length)
            };
            start &= lo == 0;
            end &= hi >= length;
            // ASCII text skips straight to the window.
            let mut offset = if ascii { lo.min(current.text.len()) } else { 0 };
            for character in current.text[offset..].chars().map(Some).chain([None]) {
                if point == Some((node, offset)) {
                    caret.reach();
                }
                let Some(character) = character.filter(|_| offset < hi) else {
                    break;
                };
                let mut buffer = [0; 2];
                let units = character.encode_utf16(&mut buffer);
                if offset < lo || offset >= hi {
                    offset += units.len();
                    continue;
                }
                let collapses = !keep
                    && !(white_space == "pre-line" && character == '\n')
                    && matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c');
                if collapses && !hidden {
                    caret.collapse(node as i64, offset as i64);
                } else if !hidden {
                    caret.emit(units, node as i64, offset as i64);
                }
                offset += units.len();
            }
            continue;
        }
        if current.tag.starts_with('#') && node != root {
            continue;
        }
        let display = display(computed, node, style.get(Prop::DISPLAY).unwrap_or("inline"));
        let host = current.content_editable();
        if node != root && !editable && host == Some(true) {
            caret.skip_editable();
        }
        if node != root && (display == "none" || !editable && host == Some(true)) {
            continue;
        }
        let block = node != root && !display.starts_with("inline") && display != "contents";
        let atom = node != root
            && (editable && host == Some(false)
                || matches!(
                    current.tag.as_str(),
                    "br" | "img"
                        | "input"
                        | "select"
                        | "button"
                        | "textarea"
                        | "video"
                        | "audio"
                        | "canvas"
                        | "iframe"
                        | "svg"
                        | "object"
                        | "embed"
                        | "meter"
                        | "progress"
                        | "terminal"
                ));
        if block {
            caret.block_edge();
        }
        if atom {
            caret.emit(
                &[if current.tag == "br" { 10 } else { 0xfffc }],
                node as i64,
                -1,
            );
            if block {
                caret.block_edge();
            }
            continue;
        }
        if block {
            steps.push(Step::Close);
        }
        let children = &current.children;
        let (lo, hi) = limits
            .iter()
            .find(|limit| limit.0 == node)
            .map_or((0, children.len()), |limit| (limit.1, limit.2));
        start &= lo == 0;
        end &= hi == children.len();
        for at in (lo..=hi).rev() {
            if point == Some((node, at)) {
                steps.push(Step::Point);
            }
            if at > lo {
                steps.push(Step::Open(children[at - 1]));
            }
        }
    }
    caret.finish([start, end])
}

// The innerText algorithm of HTML in one pass over the flat tree. With `range`, only the selected parts of
// the text nodes in it count.
fn rendered_text(
    doc: &Document,
    state: &PageState,
    computed: &style::ComputedStyles,
    root: usize,
    range: Option<&rustc_hash::FxHashMap<usize, [usize; 2]>>,
) -> String {
    let push = |text: &mut String, breaks: &mut usize, space: &mut bool, character: char| {
        if *breaks > 0 {
            if !text.is_empty() {
                text.extend(std::iter::repeat_n('\n', *breaks));
            }
            *breaks = 0;
        } else if *space
            && !matches!(character, '\n' | '\t')
            && !text.is_empty()
            && !text.ends_with(['\n', '\t'])
        {
            text.push(' ');
        }
        *space = false;
        text.push(character);
    };
    let mut text = String::new();
    let (mut breaks, mut space, mut rows, mut cells) = (0, false, false, false);
    let mut left = range.map_or(0, |range| range.len());
    enum Step {
        // A node, and whether its parent blockifies its children.
        Open(usize, bool),
        // The line breaks after a node, and the table state to restore after a table box.
        Close(usize, Option<(bool, bool)>),
    }
    let mut pending = vec![Step::Open(root, false)];
    while let Some(step) = pending.pop() {
        let (node, blockified) = match step {
            Step::Open(node, blockified) => (node, blockified),
            Step::Close(required, table) => {
                if let Some(table) = table {
                    (rows, cells) = table;
                }
                breaks = breaks.max(required);
                continue;
            }
        };
        let current = &doc.nodes[node];
        let style = &computed.values[node];
        let display = display(computed, node, style.get(Prop::DISPLAY).unwrap_or("inline"));
        // The UA sheet hides the options of a drop-down select that are not chosen, but innerText lists them.
        let display = if display == "none"
            && matches!(current.tag.as_str(), "option" | "optgroup")
            && computed.flags[node] & style::AGENT_DISPLAY != 0
            && !current.attrs.contains_key("hidden")
        {
            "block"
        } else {
            display
        };
        // Replaced elements draw no children, so their text is not rendered.
        if node != root
            && (display == "none"
                || matches!(
                    current.tag.as_str(),
                    "textarea" | "canvas" | "video" | "audio" | "iframe" | "terminal"
                ))
            || current.tag == "#comment"
        {
            continue;
        }
        let inside = range.is_none() || left > 0 && !text.is_empty();
        if current.tag == "br" {
            if inside {
                push(&mut text, &mut breaks, &mut space, '\n');
            }
            continue;
        }
        if current.tag == "#text" {
            let mut value = current.text.as_str();
            if let Some(range) = range {
                let Some(&[start, end]) = range.get(&node) else {
                    continue;
                };
                left -= 1;
                if computed.user_select[node].0 == style::UserSelect::None
                    || state.styles.inert(node)
                {
                    continue;
                }
                let (start, end) = (
                    crate::dom::text::utf16_index(value, start),
                    crate::dom::text::utf16_index(value, end),
                );
                value = &value[start..end.max(start)];
            }
            let parent = current
                .parent
                .map_or(style, |parent| &computed.values[parent]);
            if parent
                .get(Prop::VISIBILITY)
                .is_some_and(|value| matches!(value, "hidden" | "collapse"))
            {
                continue;
            }
            let white_space = parent.get(Prop::WHITE_SPACE).unwrap_or("normal");
            let keep = matches!(white_space, "pre" | "pre-wrap" | "break-spaces");
            let lines = white_space == "pre-line";
            let transformed;
            match parent.get(Prop::TEXT_TRANSFORM) {
                Some("uppercase") => {
                    transformed = value.to_uppercase();
                    value = &transformed;
                }
                Some("lowercase") => {
                    transformed = value.to_lowercase();
                    value = &transformed;
                }
                _ => {}
            }
            for character in value.chars() {
                if !keep
                    && !(lines && character == '\n')
                    && matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c')
                {
                    space = true;
                } else {
                    push(&mut text, &mut breaks, &mut space, character);
                }
            }
            continue;
        }
        let required = if display == "contents" {
            0
        } else if current.tag == "p" {
            2
        } else if blockified
            || matches!(
                display,
                "block" | "list-item" | "flex" | "grid" | "table" | "flow-root" | "table-caption"
            )
        {
            1
        } else {
            0
        };
        if node != root {
            breaks = breaks.max(required);
        }
        let table = style::table_box(style::role(computed, node));
        pending.push(Step::Close(required, table.then_some((rows, cells))));
        if table {
            (rows, cells) = (false, false);
        }
        match display {
            "table-row" => {
                if rows && inside {
                    push(&mut text, &mut breaks, &mut space, '\n');
                }
                (rows, cells) = (true, false);
            }
            "table-cell" => {
                if cells && inside {
                    push(&mut text, &mut breaks, &mut space, '\t');
                }
                cells = true;
            }
            _ => {}
        }
        if crate::render::layout::cull::skipping(doc, &state.layout, computed, node, false) {
            continue;
        }
        let blockify = display.ends_with("flex")
            || display.ends_with("grid")
            || display == "contents" && blockified;
        pending.extend(
            state
                .styles
                .shadow_walk
                .shadows
                .composed
                .get(&node)
                .unwrap_or(&current.children)
                .iter()
                .rev()
                .map(|&child| Step::Open(child, blockify)),
        );
    }
    text
}

pub(super) fn range_nodes(args: &Value, doc: &mut Document) -> Result<Value> {
    let start = args[0].as_u64().context("Expected start node")? as usize;
    let end = args[2].as_u64().context("Expected end node")? as usize;
    ensure!(
        start < doc.nodes.len() && end < doc.nodes.len(),
        "Invalid range node"
    );
    let from = args[1].as_u64().context("Expected start offset")? as usize;
    let to = args[3].as_u64().context("Expected end offset")? as usize;
    let mut positions = HashMap::new();
    let mut following = |mut id: usize| {
        loop {
            let parent = doc.nodes[id].parent?;
            let children = &doc.nodes[parent].children;
            if !positions.contains_key(&id) {
                positions.extend(
                    children
                        .iter()
                        .enumerate()
                        .map(|(index, &child)| (child, index)),
                );
            }
            if let Some(&next) = children.get(*positions.get(&id)? + 1) {
                return Some(next);
            }
            id = parent;
        }
    };
    let mut current = if doc.nodes[start].tag == "#text" {
        Some(start)
    } else {
        doc.nodes[start]
            .children
            .get(from)
            .copied()
            .or_else(|| following(start))
    };
    let stop = if doc.nodes[end].tag == "#text" {
        following(end)
    } else {
        doc.nodes[end]
            .children
            .get(to)
            .copied()
            .or_else(|| following(end))
    };
    // With `elements`, elements that the range holds whole come too, for their boxes.
    let elements = args.get(4).and_then(Value::as_bool) == Some(true);
    let mut ranges = Vec::new();
    // Like CSSOM, an element whose parent is selected too gives no box of its own.
    let mut whole = None;
    while let Some(id) = current.filter(|id| Some(*id) != stop) {
        let node = &doc.nodes[id];
        if elements
            && !node.tag.starts_with('#')
            && std::iter::successors(Some(end), |&at| doc.nodes[at].parent).all(|at| at != id)
        {
            if whole.is_none_or(|whole| {
                std::iter::successors(node.parent, |&at| doc.nodes[at].parent).all(|at| at != whole)
            }) {
                ranges.push([id, 0, 0]);
                whole = Some(id);
            }
        } else if node.tag == "#text" {
            ranges.push([
                id,
                if id == start { from } else { 0 },
                if id == end {
                    to
                } else {
                    node.text.encode_utf16().count()
                },
            ]);
        }
        current = node.children.first().copied().or_else(|| following(id));
    }
    Ok(json!(ranges))
}

pub(super) fn selection(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
) -> Result<Value> {
    // A tree change that moves the document selection leaves the selection of the focused field.
    if args[3].as_bool() == Some(true)
        && state.interaction.focus.is_some_and(|focus| {
            state.interaction.selection.contains_key(&focus)
                && matches!(doc.nodes[focus].tag.as_str(), "input" | "textarea")
        })
    {
        return Ok(Value::Null);
    }
    state.interaction.selection.clear();
    state.interaction.selection_caret = None;
    if let Some(ranges) = args[1].as_array() {
        for range in ranges {
            let node = range[0].as_u64().context("Expected selection node")? as usize;
            ensure!(node < doc.nodes.len(), "Invalid selection node");
            state.interaction.selection.insert(
                node,
                [
                    range[1].as_u64().unwrap_or(0) as usize,
                    range[2].as_u64().unwrap_or(0) as usize,
                ],
            );
            if args[2].as_bool() == Some(true) {
                state.interaction.selection_caret = Some(node);
            }
        }
    } else if id != 0 {
        state.interaction.selection.insert(
            id,
            [
                args[1].as_u64().unwrap_or(0) as usize,
                args[2].as_u64().unwrap_or(0) as usize,
            ],
        );
        if args[1] == args[2] {
            state.interaction.selection_caret = Some(id);
        }
    }
    state.dirty = true;
    Ok(Value::Null)
}

// The custom highlights in order, when that changed, and the new parts of the ranges that changed.
pub(super) fn highlights(args: &Value, doc: &mut Document, state: &mut PageState) -> Result<Value> {
    let interaction = &mut state.interaction;
    for update in args[1].as_array().context("Expected ranges")? {
        let [id, range] = [0, 1].map(|at| update[at].as_u64().unwrap_or(0) as u32);
        let highlight = interaction.highlighted.entry(id).or_default();
        for node in highlight.ranges.remove(&range).into_iter().flatten() {
            if let Some(parts) = highlight.nodes.get_mut(&node) {
                parts.retain(|part| part.0 != range);
                if parts.is_empty() {
                    highlight.nodes.remove(&node);
                }
            }
        }
        for part in update[2].as_array().into_iter().flatten() {
            let [node, start, end] = [0, 1, 2].map(|at| part[at].as_u64().unwrap_or(0) as usize);
            ensure!(node < doc.nodes.len(), "Invalid highlight node");
            let parts = highlight.nodes.entry(node).or_default();
            parts.push((range, [start, end]));
            highlight.ranges.entry(range).or_default().push(node);
        }
    }
    if let Some(order) = args[0].as_array() {
        interaction.highlights = order
            .iter()
            .map(|entry| {
                let id = entry[1].as_u64().unwrap_or(0) as u32;
                (entry[0].as_str().unwrap_or_default().into(), id)
            })
            .collect();
        let live = &interaction.highlights;
        (interaction.highlighted).retain(|id, _| live.iter().any(|(_, other)| other == id));
    }
    interaction.highlight_changes += 1;
    state.dirty = true;
    Ok(Value::Null)
}

pub(super) fn user_select(
    op: &str,
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let computed = state.restyle(doc, viewport);
    if op == "dom.userSelect" {
        return Ok(if state.styles.inert(id) {
            json!((style::UserSelect::None, id))
        } else {
            json!(computed.user_select[id])
        });
    }
    let mut range = rustc_hash::FxHashMap::default();
    let (mut first, mut last) = (None, 0);
    for item in args[1].as_array().into_iter().flatten() {
        let node = item[0].as_u64().context("Expected selection node")? as usize;
        ensure!(node < doc.nodes.len(), "Invalid selection node");
        let [start, end] = [1, 2].map(|index| item[index].as_u64().unwrap_or(0) as usize);
        range.insert(node, [start, end]);
        first.get_or_insert(node);
        last = node;
    }
    let Some(mut root) = first else {
        return Ok(json!(""));
    };
    while !std::iter::successors(Some(last), |&id| doc.nodes[id].parent).any(|id| id == root)
        && let Some(parent) = doc.nodes[root].parent
    {
        root = parent;
    }
    Ok(json!(rendered_text(
        doc,
        state,
        &computed,
        root,
        Some(&range)
    )))
}

pub(super) fn text_offset(op: &str, id: usize, args: &Value, doc: &mut Document) -> Result<Value> {
    let target = args[1].as_u64().unwrap_or(0) as usize;
    let mut offset = 0;
    let mut pending = vec![id];
    while let Some(node) = pending.pop() {
        let current = &doc.nodes[node];
        if op == "dom.textOffset" && node == target {
            let index = args[2].as_u64().unwrap_or(0) as usize;
            if current.tag == "#text" {
                return Ok(json!(offset + index));
            }
            pending.clear();
            pending.extend(current.children.iter().take(index));
            while let Some(node) = pending.pop() {
                let current = &doc.nodes[node];
                if current.tag == "#text" {
                    offset += current.text.encode_utf16().count();
                } else {
                    pending.extend(&current.children);
                }
            }
            return Ok(json!(offset));
        }
        if current.tag == "#text" {
            let length = current.text.encode_utf16().count();
            if op == "dom.textPoint" && target <= offset + length {
                return Ok(json!([node, target.saturating_sub(offset)]));
            }
            offset += length;
        } else {
            pending.extend(current.children.iter().rev());
        }
    }
    Ok(if op == "dom.textOffset" {
        json!(offset)
    } else {
        let mut node = id;
        while let Some(&child) = doc.nodes[node].children.first()
            && !doc.nodes[child].children.is_empty()
        {
            node = child;
        }
        json!([node, 0])
    })
}

pub(super) fn text_rect(
    op: &str,
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
) -> Result<Value> {
    let password = doc.nodes[id]
        .password()
        .then(|| doc.nodes[id].current_value().unwrap_or("").to_owned());
    let shown = |offset| {
        password
            .as_deref()
            .map_or(offset, |value| crate::dom::text::bullets(value, offset))
    };
    let real = |index| {
        password
            .as_deref()
            .map_or(index, |value| crate::dom::text::unbullet(value, index))
    };
    let start = shown(args[1].as_u64().unwrap_or(0) as usize);
    let end = args[2].as_u64().map_or(start, |end| shown(end as usize));
    if let Some((layout, text, range, rows, font)) = state.layout.text_layout(id) {
        let content = &text[range.clone()];
        let source = doc.nodes[id].text.as_str();
        let [dx, dy] = state.geometry.scroll_shift(&state.styles.walk, id);
        if op == "dom.textIndex" {
            let position = renderer::point(
                renderer::px(args[1].as_f64().unwrap_or(0.) as f32 - dx),
                renderer::px(args[2].as_f64().unwrap_or(0.) as f32 - dy),
            );
            let index = crate::render::paint::text::text_index(layout, text, rows, position, true)
                .unwrap_or_else(|index| index)
                .clamp(range.start, range.end);
            let offset = crate::dom::text::source_offset(source, content, index - range.start);
            return Ok(json!(real(offset)));
        }
        let mut rects = crate::render::paint::text::text_rects(
            layout,
            text,
            rows,
            [start, end]
                .map(|index| range.start + crate::dom::text::shown_index(source, content, index)),
            *font,
        );
        rects
            .iter_mut()
            .for_each(|rect| (rect[0], rect[1]) = (rect[0] + dx, rect[1] + dy));
        return Ok(json!(rects));
    }
    // Text that this frame did not lay out, like the text of a block far from the view, lays out with its
    // block on the next reflow: `null` asks the script for one, as `dom.skipped` does.
    if doc.nodes[id].parent.is_some() && state.layout.force_layout(id) {
        return Ok(Value::Null);
    }
    Ok(if op == "dom.textRect" {
        json!([])
    } else {
        json!(0)
    })
}

pub(super) fn inner_text(
    id: usize,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let computed = state.restyle(doc, viewport);
    Ok(json!(rendered_text(doc, state, &computed, id, None)))
}

pub(super) fn caret(
    id: usize,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    let computed = state.restyle(doc, viewport);
    let point = args[2]
        .as_u64()
        .map(|node| (node as usize, args[3].as_u64().unwrap_or(0) as usize));
    let window = args[4]
        .as_u64()
        .map_or(usize::MAX, |window| window as usize);
    Ok(caret_text(
        doc,
        &computed,
        id,
        args[1].as_bool().unwrap_or(false),
        point,
        window,
    ))
}
