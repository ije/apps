use crate::css::selector::ACTIVE;
use crate::css::selector::FOCUS;
use crate::css::selector::FOCUS_VISIBLE;
use crate::css::selector::FOCUS_WITHIN;
use crate::css::selector::HOVER;
use crate::css::selector::Selector;
mod document;
mod entities;
pub(crate) mod geometry;
mod interaction;
mod mirror;
mod selector;
pub(crate) mod text;
pub(crate) mod tree;
mod walk;

use crate::render::PageState;
use crate::render::media::Placeholder;
pub use document::{Document, Node, escape};
pub(crate) use geometry::Drawn;
pub use geometry::Geometry;
pub use interaction::Interaction;
pub use mirror::LayoutMirror;
pub(crate) use selector::{Dependency, Named, SelectorCache};
pub use tree::Tree;
pub(crate) use walk::{ABSENT, Walk};

use crate::css;
use crate::css::properties::Prop;
use crate::style;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) const CHECKED: u16 = 16;
pub(crate) const DIRTY: u16 = 32;
pub(crate) const MODAL: u16 = 64;
pub(crate) const POPOVER: u16 = 256;
pub(crate) const FULLSCREEN: u16 = 512;
pub(crate) const BROKEN: u16 = 1024;
// Warning: one bit for both, as only custom elements can be undefined and they are never inputs.
pub(crate) const INDETERMINATE: u16 = 4096;
pub(crate) const DEFINED: u16 = INDETERMINATE;
pub(crate) const INVALID: u16 = 32768;
pub(crate) const TOP_LAYER: u16 = MODAL | POPOVER | FULLSCREEN;
pub(crate) const LEAVING: u16 = TOP_LAYER << 5;

// A `contenteditable` value makes the element editable or not, or inherits when `None`.
pub(crate) fn editable_value(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        (value.is_empty()
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("plaintext-only"))
        .then_some(true)
    }
}

static PLACEHOLDERS: LazyLock<Mutex<HashMap<u64, Placeholder>>> = LazyLock::new(Default::default);

fn new_canvas(node: &Node) -> Result<crate::render::canvas::Canvas> {
    let size = |name, default| {
        node.attrs
            .get(name)
            .and_then(|value| crate::render::dimension(value, true))
            .map_or(default, |value| value as u32)
    };
    let mut canvas = crate::render::canvas::Canvas::new(size("width", 300), size("height", 150))?;
    canvas.present = node.tag == "canvas";
    Ok(canvas)
}

// Added to the node of a drawable element, this draws its snapshot and not the image of the node.
const SNAPSHOT: usize = 1 << 32;

fn parsed(state: &mut PageState, source: &Value) -> Result<Rc<[Selector]>> {
    let source = source.as_str().context("Expected selector")?;
    if let Some(found) = state.styles.queries.get(source) {
        return Ok(found.clone());
    }
    let parsed: Rc<[Selector]> = Selector::try_parse(source)?.into();
    // Scripts repeat a few selectors, so a full cache holds generated ones.
    if state.styles.queries.len() == 256 {
        state.styles.queries.clear();
    }
    state.styles.queries.insert(source.into(), parsed.clone());
    Ok(parsed)
}

pub fn call(
    op: &str,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    if op == "dom.scrollEvents" {
        let changed = state.geometry.scroll_events();
        let mut changed = json!(changed);
        if !state.layout.skip_events.is_empty()
            && let Some(list) = changed.as_array_mut()
        {
            list.push(json!(
                std::mem::take(&mut state.layout.skip_events)
                    .into_iter()
                    .flat_map(|(id, skipped)| [id, usize::from(skipped)])
                    .collect::<Vec<_>>()
            ));
        }
        return Ok(changed);
    }
    if op == "dom.decoded" {
        let decoded = std::mem::take(
            &mut *state
                .media
                .decoded
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let mut results = Vec::with_capacity(decoded.len());
        for (id, request, result) in decoded {
            if state.media.decoding.get(&id) != Some(&request) {
                results.push(json!([request, null]));
                continue;
            }
            state.media.decoding.remove(&id);
            state.dirty |= result.is_ok();
            if result.is_ok() {
                state.layout.relayout(id);
            }
            results.push(json!([
                request,
                match result {
                    Ok(Ok(image)) => {
                        let size = image.size();
                        state
                            .layout
                            .measurements
                            .naturals
                            .insert(id, [size.width as u32 as f32, size.height as u32 as f32]);
                        state.media.svg_images.remove(&id);
                        state
                            .media
                            .retired
                            .extend(state.media.images.insert(id, Arc::new(image)));
                        json!([size.width as u32, size.height as u32])
                    }
                    Ok(Err((text, size))) => {
                        state.layout.measurements.naturals.insert(id, size);
                        state.media.retired.extend(state.media.images.remove(&id));
                        state.media.svg_images.insert(id, (text.into(), size));
                        json!(size)
                    }
                    Err(error) => json!(error.to_string()),
                }
            ]));
        }
        return Ok(json!(results));
    }
    if op == "dom.frames" {
        let frames = std::mem::take(
            &mut *state
                .media
                .frames
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let mut results = Vec::with_capacity(frames.len());
        if state.media.retired.is_empty() {
            state.media.retired_canvases.clear();
        }
        for (node, image) in frames {
            let size = image.size();
            results.push(json!([node, size.width as u32, size.height as u32]));
            let old = state.media.images.insert(node, image);
            state.media.retire(node, old);
            state.dirty = true;
        }
        return Ok(json!(results));
    }
    if op == "dom.wheel" {
        state.wheel = true;
        return Ok(Value::Null);
    }
    if op == "dom.iterations" {
        state.styles.motion.listen_iterations();
        return Ok(Value::Null);
    }
    if op == "dom.cssControl" {
        let time = args[2].as_f64().unwrap_or(0.);
        if !time.is_finite() {
            return Ok(json!(false));
        }
        let styles = state.restyle(doc, viewport);
        state.dirty = true;
        return Ok(json!(state.styles.motion.css_control(
            args[0].as_u64().unwrap_or(0) as u32,
            args[1].as_str().unwrap_or_default(),
            time,
            &styles,
        )));
    }
    if op == "dom.easing" {
        let progress = args[1].as_f64().unwrap_or(0.);
        return Ok(json!(
            css::value::easing::Easing::parse(args[0].as_str().unwrap_or("linear"))
                .map_or(progress, |easing| easing.at(progress, args[2] == true))
        ));
    }
    if op == "dom.animation" {
        let id = args[0].as_u64().context("Expected an animation")? as u32;
        state.dirty = true;
        let old = state.styles.motion.unscript(id);
        if let Some((node, _)) = &old {
            state.styles.animating.remove(node);
            state.styles.animating_pseudos.remove(node);
        }
        let spec = &args[2];
        let Some(node) = args[1]
            .as_u64()
            .map(|node| node as usize)
            .filter(|&node| spec.is_object() && node < doc.nodes.len())
        else {
            return Ok(Value::Null);
        };
        state.styles.animating.remove(&node);
        state.styles.animating_pseudos.remove(&node);
        let frames: Vec<(f64, &str, &str)> = spec["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|frame| {
                Some((
                    frame[0].as_f64()?,
                    frame[1].as_str()?,
                    frame[2].as_str().unwrap_or("linear"),
                ))
            })
            .collect();
        let number = |index: usize| spec["timing"][index].as_f64().unwrap_or(0.);
        let iterations = number(3);
        let animation = style::animation::timing::Scripted::new(
            id,
            &frames,
            [
                number(0) / 1000.,
                number(1) / 1000.,
                number(2) / 1000.,
                if iterations < 0. {
                    f64::INFINITY
                } else {
                    iterations
                },
                number(4),
            ],
            [
                spec["direction"].as_str().unwrap_or("normal"),
                spec["fill"].as_str().unwrap_or("none"),
                spec["easing"].as_str().unwrap_or("linear"),
                spec["composite"].as_str().unwrap_or("replace"),
            ],
            match spec["pseudo"].as_str() {
                Some("::before") => 1,
                Some("::after") => 2,
                _ => 0,
            },
        );
        state.styles.motion.script(
            node,
            animation,
            spec["time"].as_f64().unwrap_or(0.) / 1000.,
            spec["rate"].as_f64(),
            old.map(|(_, old)| old)
                .filter(|_| !spec["frames"].is_array()),
        );
        return Ok(Value::Null);
    }
    // A request only needs a tick: the page draws when the callbacks change it.
    if op == "dom.requestFrame" {
        state.frame_requested = true;
        return Ok(json!(state.fonts.system.is_some()));
    }
    if op == "dom.requestPaint" {
        state.paint_requested = true;
        state.dirty = true;
        return Ok(Value::Null);
    }
    if op == "dom.mediaMatches" {
        let query = args[0].as_str().context("Expected a media query")?;
        let source = format!("@media {query} {{}}");
        let theme = state.theme();
        let sheet = state
            .styles
            .sheets
            .entry(source.as_str().into())
            .or_insert_with(|| Rc::new(css::sheet::Stylesheet::parse(&source)));
        return Ok(sheet.matches_media(viewport[0], viewport[1], theme).into());
    }
    if op == "dom.properties" {
        return Ok(json!(css::properties::Prop::names()));
    }
    if op == "dom.aliases" {
        let aliases = css::properties::Prop::names()
            .iter()
            .map(|name| format!("-webkit-{name}"));
        return Ok(json!(
            aliases
                .filter(|alias| css::properties::Prop::unprefixed(alias).is_some())
                .collect::<Vec<_>>()
        ));
    }
    if op == "dom.path" {
        return Ok(json!(crate::render::canvas::svg_path(
            args[0].as_str().unwrap_or_default()
        )));
    }
    if op == "dom.specified" {
        return Ok(json!(css::value::specified::specified(
            args[0].as_str().unwrap_or_default(),
            args[1].as_str().unwrap_or_default()
        )));
    }
    if op == "dom.declare" {
        return Ok(json!(
            css::sheet::declare(args[0].as_str().unwrap_or_default()).concat()
        ));
    }
    if matches!(
        op,
        "dom.constructed" | "dom.adopt" | "dom.supports" | "dom.registerProperty"
    ) {
        let text = args[2].as_str().unwrap_or_default();
        let result = match op {
            "dom.supports" => {
                let name = args[0].as_str().context("Expected CSS text")?;
                return Ok(json!(match args[1].as_str() {
                    Some(value) =>
                        css::value::grammar::supports(&name.to_ascii_lowercase(), value)
                            == Some(true),
                    None => css::sheet::parser::supports_condition(name),
                }));
            }
            "dom.registerProperty" => {
                let name = args[0].as_str().context("Expected a name")?;
                let syntax = args[1].as_str().unwrap_or("*");
                let initial = args[3].as_str();
                let rule = format!(
                    "@property {name}{{syntax:{};inherits:{};{}}}",
                    json!(syntax),
                    args[2].as_bool().unwrap_or(false),
                    initial.map_or(String::new(), |value| format!("initial-value:{value}"))
                );
                let registered = css::sheet::Stylesheet::parse(&rule).property();
                // `Prop::new` leaks the names it interns, so only a valid new registration interns one.
                match registered {
                    _ if !name.starts_with("--") => Err("SyntaxError"),
                    _ if css::properties::Prop::find(name)
                        .is_some_and(|prop| state.styles.cache.scripted.contains_key(&prop)) =>
                    {
                        Err("InvalidModificationError")
                    }
                    Some(value) => {
                        state
                            .styles
                            .cache
                            .scripted
                            .insert(css::properties::Prop::new(name), value);
                        Ok(Value::Null)
                    }
                    None => Err("SyntaxError"),
                }
            }
            "dom.adopt" => {
                let root = args[0].as_u64().context("Expected a root")? as usize;
                let ids = args[1]
                    .as_array()
                    .context("Expected sheets")?
                    .iter()
                    .filter_map(|id| Some(id.as_u64()? as u32))
                    .collect::<Vec<_>>();
                if ids.is_empty() {
                    state.styles.cache.adopted.remove(&root);
                } else {
                    state.styles.cache.adopted.insert(root, ids);
                }
                Ok(Value::Null)
            }
            _ => {
                let id = args[0].as_u64().context("Expected a sheet")? as u32;
                match args[1].as_str().unwrap_or_default() {
                    "parse" => return Ok(css::sheet::Stylesheet::parse(text).cssom()),
                    kind @ ("replace" | "load") => {
                        let sheet = css::sheet::Stylesheet::parse(text);
                        let tree = if kind == "load" {
                            Value::Null
                        } else {
                            sheet.cssom()
                        };
                        state.styles.cache.constructed.insert(id, Rc::new(sheet));
                        Ok(tree)
                    }
                    "drop" => {
                        state.styles.cache.constructed.remove(&id);
                        Ok(Value::Null)
                    }
                    kind => {
                        let (path, top) = path(kind, &args[3]);
                        state
                            .styles
                            .cache
                            .edit_constructed(id, top, |sheet| sheet.edit(&path, edit(kind, text)))
                    }
                }
            }
        };
        state.styles.invalidate();
        state.dirty = true;
        return Ok(result.unwrap_or_else(|error| json!({ "error": error })));
    }
    if op == "dom.release" {
        let wrapped = args[1].as_array().context("Expected live nodes")?;
        state.tree.note_created(wrapped.len());
        let dead = args[0].as_array().context("Expected dead nodes")?;
        let ids = |value: &Value| value.as_u64().map(|id| id as usize);
        state.tree.mark_live(
            doc,
            wrapped.iter().filter_map(ids),
            dead.iter().filter_map(ids),
        );
        let (canvases, freed) = state.release(doc);
        return Ok(json!([canvases, freed]));
    }
    if op == "dom.rangeNodes" {
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
                    std::iter::successors(node.parent, |&at| doc.nodes[at].parent)
                        .all(|at| at != whole)
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
        return Ok(json!(ranges));
    }
    if op == "dom.hitTest" {
        ensure!(!doc.nodes.is_empty(), "Document is unavailable");
        let x = args[0].as_f64().context("Expected x")? as f32;
        let y = args[1].as_f64().context("Expected y")? as f32;
        let computed = state.restyle(doc, viewport);
        let (first, caret) = (args[2].as_bool(), args[3].as_bool());
        return Ok(crate::render::hit::hit_test(
            doc,
            state,
            &computed,
            viewport,
            [x, y],
            first.unwrap_or(false),
            caret.unwrap_or(false),
        ));
    }
    if matches!(op, "dom.focus" | "dom.hover" | "dom.active") {
        let ids = args[0]
            .as_array()
            .context("Expected nodes")?
            .iter()
            .map(|id| {
                id.as_u64()
                    .map(|id| id as usize)
                    .filter(|&id| id < doc.nodes.len())
                    .context("Invalid node id")
            })
            .collect::<Result<Vec<_>>>()?;
        let (index, flag) = match op {
            "dom.focus" => (0, FOCUS_WITHIN),
            "dom.hover" => (1, HOVER),
            _ => (2, ACTIVE),
        };
        let focus = if op == "dom.focus" { FOCUS } else { 0 };
        let shown = args[2].as_bool() != Some(true);
        let used = state.styles.cache.states & (flag | focus) != 0;
        let control = |id: &usize| {
            doc.nodes.get(*id).is_some_and(|node| {
                matches!(
                    node.tag.as_str(),
                    "button" | "input" | "select" | "textarea"
                )
            })
        };
        let repaint = focus == 0
            && (ids.iter().any(control) || state.interaction.states[index].iter().any(control));
        if op == "dom.focus" {
            for &id in state
                .interaction
                .focus
                .iter()
                .chain(&state.interaction.states[0])
            {
                if let Some(node) = doc.nodes.edit(id) {
                    node.state &= !(FOCUS | FOCUS_VISIBLE);
                }
            }
            state.interaction.focus = ids.first().copied();
            if let Some(focus) = state.interaction.focus.filter(|_| shown) {
                if let Some(node) = doc.nodes.edit(focus) {
                    node.state |= if args[1].as_bool() == Some(false) {
                        FOCUS
                    } else {
                        FOCUS | FOCUS_VISIBLE
                    };
                }
                // Like Chrome, the hosts of the shadow trees around the focused element match `:focus`. A host of a
                // slotted element does not, so the walk follows tree parents, not the composed path.
                let mut at = focus;
                while let Some(host) = {
                    while let Some(up) = doc.nodes[at].parent {
                        at = up;
                    }
                    ids.iter()
                        .copied()
                        .find(|host| state.tree.shadows.get(host) == Some(&at))
                } {
                    if let Some(node) = doc.nodes.edit(host) {
                        node.state |= FOCUS;
                    }
                    at = host;
                }
            }
        }
        for &id in &state.interaction.states[index] {
            if let Some(node) = doc.nodes.edit(id) {
                node.state &= !flag;
            }
        }
        for &id in ids.iter().filter(|_| shown) {
            if let Some(node) = doc.nodes.edit(id) {
                node.state |= flag;
            }
        }
        state.interaction.states[index] = ids;
        if used {
            state.styles.invalidate();
        }
        state.dirty |= used || focus != 0 || repaint;
        return Ok(Value::Null);
    }
    if op == "dom.target" {
        let target = args[0]
            .as_u64()
            .map(|id| id as usize)
            .filter(|&id| id < doc.nodes.len());
        for id in [doc.target, target].into_iter().flatten() {
            doc.nodes.edit(id);
        }
        doc.target = target;
        state.styles.invalidate();
        state.dirty = true;
        return Ok(Value::Null);
    }
    if op == "dom.reserve" {
        let ids: Vec<_> = (0..args[0].as_u64().context("Expected count")?.min(4096))
            .map(|_| state.allocate(doc, Node::default()))
            .collect();
        state.tree.reserved.extend(&ids);
        return Ok(json!(ids));
    }
    let id = args[0].as_u64().context("Expected node id")? as usize;
    ensure!(id < doc.nodes.len(), "Invalid node id");
    match op {
        "dom.assignedNodes" => {
            let mut roots = HashSet::new();
            let mut slotted = HashMap::new();
            let mut add = |slot: usize, slotted: &mut HashMap<usize, Vec<usize>>| {
                let mut top = slot;
                while let Some(parent) = doc.nodes[top].parent {
                    top = parent;
                }
                if let Some(&host) = state.tree.hosts.get(&top)
                    && roots.insert(top)
                {
                    slotted.extend(tree::slots(doc, [(host, top)]));
                }
            };
            add(id, &mut slotted);
            let mut nodes = slotted.get(&id).cloned().unwrap_or_default();
            if args[1].as_bool() == Some(true) && slotted.contains_key(&id) {
                if nodes.is_empty() {
                    nodes = doc.nodes[id].children.clone();
                }
                let mut pending: Vec<_> = nodes.into_iter().rev().collect();
                nodes = Vec::new();
                while let Some(child) = pending.pop() {
                    if doc.nodes[child].tag == "slot" {
                        add(child, &mut slotted);
                    }
                    if let Some(assigned) = slotted.get(&child) {
                        pending.extend(
                            if assigned.is_empty() {
                                &doc.nodes[child].children
                            } else {
                                assigned
                            }
                            .iter()
                            .rev(),
                        );
                    } else if doc.nodes[child].tag != "#comment" {
                        nodes.push(child);
                    }
                }
            }
            Ok(json!(nodes))
        }
        "dom.tree" => Ok(tree(doc, id, true)),
        "dom.source" => {
            let node = &doc.nodes[id];
            Ok(json!(match &node.stylesheet {
                Some(sheet) if node.tag == "#text" => sheet.source(),
                _ => node.text.clone(),
            }))
        }
        "dom.matches" => Ok(json!(parsed(state, &args[1])?.iter().any(|s| {
            s.matches_cached(doc, id, Some(id), &mut SelectorCache::default())
        }))),
        "dom.closest" => {
            let selectors = parsed(state, &args[1])?;
            let mut memo = SelectorCache::default();
            let mut current = Some(id);
            while let Some(node) = current.filter(|&node| !doc.nodes[node].tag.starts_with('#')) {
                if selectors
                    .iter()
                    .any(|s| s.matches_cached(doc, node, Some(id), &mut memo))
                {
                    return Ok(json!(node));
                }
                current = doc.nodes[node].parent;
            }
            Ok(Value::Null)
        }
        "dom.queryWithin" => {
            let selectors = parsed(state, &args[1])?;
            let first = args[2].as_bool().unwrap_or(false);
            let mut pending: Vec<_> = doc.nodes[id].children.iter().rev().copied().collect();
            let mut found = Vec::new();
            let mut memo = SelectorCache::default();
            while let Some(node) = pending.pop() {
                if selectors
                    .iter()
                    .any(|s| s.matches_cached(doc, node, Some(id), &mut memo))
                {
                    if first {
                        return Ok(json!(node));
                    }
                    found.push(node);
                }
                if doc.nodes[node].tag != "template" {
                    pending.extend(doc.nodes[node].children.iter().rev());
                }
            }
            Ok(if first { Value::Null } else { json!(found) })
        }
        "dom.html" => {
            let source = args[1].as_str().context("Expected HTML")?;
            let tag = doc.nodes[id].tag.as_str();
            let form = || {
                std::iter::successors(Some(id), |&at| doc.nodes[at].parent)
                    .any(|at| doc.nodes[at].tag == "form")
            };
            let parsed = match args[2].as_str() {
                _ if tag == "#document" => Document::parse(source),
                Some(context) => Document::build(source, Some(context), form()),
                // Shadow roots and template contents parse in `template` context, so the `<tr>` rows that
                // Lit renders are kept.
                None if tag.starts_with('#') => Document::fragment(source, "template"),
                None => Document::build(source, Some(tag), form()),
            };
            for child in std::mem::take(&mut doc.nodes[id].children) {
                doc.nodes[child].parent = None;
                state.tree.detached.push(child);
                state.tree.removed.push(child);
            }
            doc.nodes[id].text.clear();
            let mut ids = Vec::with_capacity(parsed.nodes.len());
            ids.push(id);
            for _ in 1..parsed.nodes.len() {
                ids.push(state.tree.slot(doc));
            }
            doc.nodes[id].children = parsed.nodes[0]
                .children
                .iter()
                .map(|&child| ids[child])
                .collect();
            let detached = state.tree.detached.len();
            for (index, mut node) in parsed.nodes.into_iter().enumerate().skip(1) {
                node.parent = node.parent.map(|parent| ids[parent]);
                for child in &mut node.children {
                    *child = ids[*child];
                }
                state.place(doc, ids[index], node);
            }
            state.tree.detached.truncate(detached);
            state.dirty = true;
            Ok(tree(doc, id, false))
        }
        "dom.cssAnimations" => {
            let styles = state.restyle(doc, viewport);
            let mut nodes = Vec::new();
            let mut pending = vec![id];
            while let Some(node) = pending.pop() {
                nodes.push(node);
                if args[1].as_bool() == Some(true) {
                    pending.extend(doc.nodes[node].children.iter().rev());
                    pending.extend(state.tree.shadows.get(&node));
                }
            }
            // Without a layout, answer only when the frame would start nothing here.
            if args[3].as_bool() == Some(true) && !state.styles.motion.settled(&nodes, &styles) {
                return Ok(Value::Null);
            }
            Ok(json!(state.styles.motion.css(
                &nodes,
                args[2].as_bool() == Some(true),
                &styles
            )))
        }
        "dom.animated" => {
            let styles = state.restyle(doc, viewport);
            Ok(state
                .styles
                .motion
                .scripted(id, 0, &styles, args[1].as_u64().unwrap_or(0) as u32)
                .map_or(Value::Null, |values| {
                    values
                        .iter()
                        .map(|(key, value)| (key.name().to_owned(), json!(&**value)))
                        .collect::<serde_json::Map<_, _>>()
                        .into()
                }))
        }
        "dom.computed" => {
            let styles = state.restyle(doc, viewport);
            let pseudo = args[2].as_str().unwrap_or_default().trim_start_matches(':');
            if let Some(style) = styles.values.get(id) {
                state.styles.motion.read(id, style);
            }
            let slot = match pseudo {
                "" => Some(0),
                "before" => Some(1),
                "after" => Some(2),
                _ => None,
            };
            let scripted =
                slot.and_then(|slot| state.styles.motion.scripted(id, slot, &styles, u32::MAX));
            // The compositor can play animations between frames, so they are sampled now. As in Chrome, values that
            // move boxes stay those of the last frame, which match its layout.
            let current = state.styles.animating.get(&id).and_then(|last| {
                let now = std::time::Instant::now();
                let current = state
                    .styles
                    .motion
                    .timeline(id, 0, &styles, &[now], &[])
                    .pop()?;
                let paints = current.iter().all(|(prop, value)| {
                    crate::render::composited(prop) || last.get(prop) == Some(&**value)
                });
                paints.then_some(current)
            });
            let mut sampled = None;
            let backdrop = (pseudo == "backdrop")
                .then(|| state.styles.motion.backdrop(id, styles.backdrops.get(&id)?))
                .flatten();
            let style = match pseudo {
                "" => Some(current.as_ref().unwrap_or_else(|| {
                    &**state
                        .styles
                        .animating
                        .get(&id)
                        .or_else(|| styles.authored.get(&id))
                        .unwrap_or(&styles.values[id])
                })),
                "before" | "after" => state
                    .styles
                    .animating_pseudos
                    .get(&id)
                    .or_else(|| styles.pseudos.get(&id))
                    .map(|values| &values[usize::from(pseudo == "after")]),
                "backdrop" => backdrop
                    .as_ref()
                    .or_else(|| styles.backdrops.get(&id).map(|style| &**style)),
                "selection" => styles.selections.get(&id).map(|style| &**style),
                // The button of a file input shares the slot of the scrollbar track.
                "file-selector-button" | "-webkit-file-upload-button"
                    if doc.nodes[id].tag == "input"
                        && doc.nodes[id].input_type().eq_ignore_ascii_case("file") =>
                {
                    styles
                        .scrollbars
                        .get(&id)
                        .and_then(|styles| styles[1].as_ref())
                }
                "marker" | "first-letter" | "first-line" | "placeholder" => {
                    let index = ["marker", "first-letter", "first-line"]
                        .iter()
                        .position(|name| *name == pseudo)
                        .unwrap_or(3);
                    styles
                        .text_pseudos
                        .get(&id)
                        .and_then(|styles| styles[index].as_ref())
                }
                _ => None,
            };
            let style = match (style, scripted) {
                (Some(style), Some(values)) => {
                    let mut style = style.clone();
                    style.extend(values.iter().map(|(key, value)| (key, value.clone())));
                    Some(&*sampled.insert(style))
                }
                (style, _) => style,
            };
            let mut full = None;
            let style = match style {
                Some(style)
                    if style.vars().is_empty()
                        && !styles.vars[id].is_empty()
                        && args[1].as_str().is_none_or(|name| name.starts_with("--")) =>
                {
                    let mut copy = style.clone();
                    copy.set_vars(styles.vars[id].clone());
                    Some(&*full.insert(copy))
                }
                style => style,
            };
            let Some(style) = style.filter(|style| !style.is_empty()) else {
                return Ok(json!(if args[1].is_string() {
                    json!(
                        if args[1] == "content" && matches!(pseudo, "before" | "after") {
                            "none"
                        } else {
                            ""
                        }
                    )
                } else {
                    json!([])
                }));
            };
            let mut units = styles.lengths[id];
            // Container units: the width of the nearest container, the height of the nearest size container.
            let mut sizes = [None, None];
            let queried = !state.layout.containers.is_empty()
                && style.own().any(|(_, value)| value.contains("cq"));
            for node in std::iter::successors(doc.nodes[id].parent, |&node| doc.nodes[node].parent)
                .take_while(|_| queried)
            {
                let kind = styles.values[node].get(Prop::CONTAINER_TYPE);
                if let (Some(kind @ ("size" | "inline-size")), Some(size)) =
                    (kind, state.layout.containers.get(&node))
                {
                    sizes[0] = sizes[0].or(Some(size[0]));
                    if kind == "size" {
                        sizes[1] = Some(size[1]);
                        break;
                    }
                }
            }
            if sizes[0].is_some() {
                units.container = Some([0, 1].map(|axis| sizes[axis].unwrap_or(viewport[axis])));
            }
            let shown = |node: usize| {
                state.geometry.mirror.get(&node).is_some() && state.geometry.mirror.shown(node)
            };
            // Padding, plus borders when `borders`, of each side, with percentages of `base`.
            let edges = |style: &css::properties::Style,
                         units: &css::value::units::Lengths,
                         base,
                         borders| {
                let mut edges = units.edges(style, css::properties::PADDING, base);
                for (edge, [width, kind, _]) in edges.iter_mut().zip(css::properties::BORDERS) {
                    if borders
                        && style
                            .get(kind)
                            .is_some_and(|kind| !matches!(kind, "none" | "hidden"))
                    {
                        *edge += style
                            .get(width)
                            .map_or(Some(3.), |width| units.resolve(width, base))
                            .unwrap_or(0.)
                            .max(0.);
                    }
                }
                edges
            };
            let position = style.get(Prop::POSITION).unwrap_or("static");
            // The containing block: the parent's content box, or for positioned boxes the padding box of the
            // nearest positioned or transformed ancestor. `None` is the viewport.
            // `::before` and `::after` are children of their element.
            let generated = matches!(pseudo, "before" | "after");
            let start = if generated {
                Some(id)
            } else {
                doc.nodes[id].parent
            };
            let block =
                match position {
                    "fixed" => None,
                    "absolute" => std::iter::successors(start, |&node| doc.nodes[node].parent)
                        .find(|&node| {
                            styles.values[node]
                                .get(Prop::POSITION)
                                .is_some_and(|value| value != "static")
                                || styles.values[node]
                                    .get(Prop::TRANSFORM)
                                    .is_some_and(|value| value != "none")
                        }),
                    _ => start,
                }
                .and_then(|node| {
                    let rect = *state.geometry.mirror.get(&node)?;
                    let (style, units) = (&styles.values[node], &styles.lengths[node]);
                    let outer = edges(style, units, rect[2], true);
                    let inner =
                        state.layout.cull.widths.get(&node).copied().unwrap_or([
                            rect[2] - outer[1] - outer[3],
                            rect[3] - outer[0] - outer[2],
                        ]);
                    let padding = if position == "absolute" {
                        edges(style, units, inner[0], false)
                    } else {
                        [0.; 4]
                    };
                    Some([
                        rect[0] + outer[3] - padding[3],
                        rect[1] + outer[0] - padding[0],
                        inner[0] + padding[1] + padding[3],
                        inner[1] + padding[0] + padding[2],
                    ])
                })
                .unwrap_or([0., 0., viewport[0], viewport[1]]);
            // A grid item takes percentages from its grid area.
            let area = (!generated
                && !matches!(position, "absolute" | "fixed")
                && !style::table_part(doc, &styles, id))
            .then(|| {
                // The box parent, past `display: contents` boxes.
                std::iter::successors(state.tree.flat_parent(doc, &styles, id), |&parent| {
                    state.tree.flat_parent(doc, &styles, parent)
                })
                .find(|&parent| styles.values[parent].get(Prop::DISPLAY) != Some("contents"))
            })
            .flatten()
            .filter(|parent| state.layout.tracks.contains_key(parent))
            .and_then(|_| state.layout.areas.get(&id).copied());
            let block = area.map_or(block, |[width, height]| [block[0], block[1], width, height]);
            let container = || [block[2], block[3]];
            let inline = !doc.nodes[id].tag.starts_with('#')
                && style
                    .get(Prop::DISPLAY)
                    .is_none_or(|display| display == "inline")
                && !matches!(
                    doc.nodes[id].tag.as_str(),
                    "img"
                        | "svg"
                        | "canvas"
                        | "video"
                        | "audio"
                        | "iframe"
                        | "embed"
                        | "object"
                        | "input"
                        | "select"
                        | "textarea"
                        | "button"
                );
            // The untransformed border box. Inline boxes and boxes that are not rendered have none, so their
            // percentages stay as they are.
            // The box of a rendered `::before` or `::after`: its place, size and padding plus border.
            let pseudo_box = (generated
                && !inline
                && shown(id)
                && !matches!(style.get(Prop::DISPLAY), Some("none" | "contents"))
                && style
                    .get(Prop::CONTENT)
                    .is_some_and(|content| !matches!(content, "none" | "normal")))
            .then(|| {
                let key =
                    crate::render::inline::atom_key(id, if pseudo == "after" { 2 } else { 1 });
                state.layout.pseudo_boxes.get(&key).copied()
            })
            .flatten();
            let size = (pseudo.is_empty() && !inline && shown(id))
                .then(|| {
                    let inner = state.layout.cull.widths.get(&id)?;
                    let sides = edges(style, &units, block[2], true);
                    Some([
                        inner[0] + sides[1] + sides[3],
                        inner[1] + sides[0] + sides[2],
                    ])
                })
                .flatten()
                .or_else(|| {
                    (pseudo.is_empty() && !inline && shown(id))
                        .then(|| {
                            state
                                .geometry
                                .mirror
                                .get(&id)
                                .map(|rect| [rect[2], rect[3]])
                        })
                        .flatten()
                })
                .or_else(|| pseudo_box.map(|rect| [rect[2], rect[3]]));
            let registry = state.styles.cache.registry();
            let parent = start.map(|parent| &*styles.values[parent]);
            let flow = start.is_some_and(|parent| style::table_box(style::role(&styles, parent)))
                || parent.is_none_or(|parent| {
                    parent.get(Prop::DISPLAY).is_none_or(|display| {
                        !display.ends_with("flex") && !display.ends_with("grid")
                    })
                });
            let value = |name: &str| -> String {
                let name = css::value::resolved::physical_name(style, name);
                let get = |name| {
                    css::value::resolved::resolved(style, &units, name, size, &container, &registry)
                };
                let value = get(&name);
                match (&*name, value.as_str()) {
                    ("display", display_value) if pseudo.is_empty() => {
                        display(&styles, id, display_value).to_owned()
                    }
                    ("gap" | "row-gap" | "column-gap", _)
                        if style::table_box(style::role(&styles, id)) =>
                    {
                        "normal".to_owned()
                    }
                    // Like Chrome, an element with no box, like `display: contents` or an element inside a
                    // `display: none` box of the flat tree, has no transform.
                    ("transform", transform)
                        if transform != "none"
                            && size.is_none()
                            && (style.get(Prop::DISPLAY) == Some("contents")
                                || std::iter::successors(
                                    state.tree.flat_parent(doc, &styles, id),
                                    |&node| state.tree.flat_parent(doc, &styles, node),
                                )
                                .any(|node| {
                                    styles.values[node].get(Prop::DISPLAY) == Some("none")
                                })) =>
                    {
                        "none".to_owned()
                    }
                    // As in Chrome, an SVG shape with no size has none, and a percentage is of its viewport: the
                    // `viewBox` or the box of its `<svg>`.
                    ("width" | "height", given)
                        if matches!(doc.nodes[id].tag.as_str(), "rect" | "image")
                            && (given == "auto" || given.contains('%')) =>
                    {
                        let axis = usize::from(&*name == "height");
                        let svg = std::iter::successors(doc.nodes[id].parent, |&node| {
                            doc.nodes[node].parent
                        })
                        .find(|&node| doc.nodes[node].tag == "svg");
                        let viewport = svg.and_then(|svg| {
                            let view_box: Vec<f32> = doc.nodes[svg]
                                .attrs
                                .get("viewBox")?
                                .split([' ', ','])
                                .filter_map(|part| part.parse().ok())
                                .collect();
                            view_box.get(2 + axis).copied()
                        });
                        let viewport = viewport.or_else(|| {
                            svg.and_then(|svg| state.geometry.mirror.get(&svg))
                                .map(|rect| rect[2 + axis])
                        });
                        match viewport.and_then(|base| units.resolve(given, base)) {
                            Some(length) => {
                                format!("{}px", css::value::tokens::number(f64::from(length)))
                            }
                            None => "0px".to_owned(),
                        }
                    }
                    ("min-width" | "min-height", "auto") if flow => "0px".to_owned(),
                    ("top" | "right" | "bottom" | "left", "auto")
                        if position != "static" && position != "sticky" =>
                    {
                        let (opposite, axis) = match &*name {
                            "top" => ("bottom", 1),
                            "bottom" => ("top", 1),
                            "left" => ("right", 0),
                            _ => ("left", 0),
                        };
                        if position == "relative" {
                            let other = get(opposite);
                            return match other
                                .strip_suffix("px")
                                .and_then(|px| px.parse::<f32>().ok())
                            {
                                Some(px) if px != 0. => {
                                    format!("{}px", css::value::tokens::number(f64::from(-px)))
                                }
                                _ => "0px".to_owned(),
                            };
                        }
                        let corner = if generated {
                            // The layout place of the pseudo-element is from the border box of its element.
                            let (Some([x, y, ..]), Some(&[left, top, ..])) =
                                (pseudo_box, state.geometry.mirror.get(&id))
                            else {
                                return value;
                            };
                            [left + x, top + y]
                        } else {
                            let (Some(&[x, y, width, height]), Some(size)) =
                                (state.geometry.mirror.get(&id), size)
                            else {
                                return value;
                            };
                            // The bounds hold the transformed box, whose center is the transformed center of the box.
                            let bounds = renderer::Rect::new(
                                renderer::point(renderer::px(0.), renderer::px(0.)),
                                renderer::size(renderer::px(size[0]), renderer::px(size[1])),
                            );
                            let center = match crate::css::value::transform::transform(
                                style, &units, bounds, None,
                            ) {
                                Some(m) if m[3] == 0. && m[7] == 0. && m[15] == 1. => {
                                    let [cx, cy] = [size[0] / 2., size[1] / 2.];
                                    [m[0] * cx + m[4] * cy + m[12], m[1] * cx + m[5] * cy + m[13]]
                                }
                                _ => [size[0] / 2., size[1] / 2.],
                            };
                            [x + width / 2. - center[0], y + height / 2. - center[1]]
                        };
                        let Some(size) = size else {
                            return value;
                        };
                        let margin = units.edges(style, css::properties::MARGIN, block[2]);
                        let start = corner[axis] - margin[3 - 3 * axis] - block[axis];
                        let used = if name == "top" || name == "left" {
                            start
                        } else {
                            block[2 + axis]
                                - start
                                - size[axis]
                                - margin[3 - 3 * axis]
                                - margin[1 + axis]
                        };
                        format!(
                            "{}px",
                            css::value::tokens::number(f64::from((used * 64.).round() / 64.))
                        )
                    }
                    ("margin-left" | "margin-right", "auto") if size.is_some() => {
                        if !flow || position == "absolute" || position == "fixed" {
                            return "0px".to_owned();
                        }
                        let left = name == "margin-left";
                        let other = get(if left { "margin-right" } else { "margin-left" });
                        let other = other
                            .strip_suffix("px")
                            .and_then(|px| px.parse::<f32>().ok());
                        let free = block[2] - size.unwrap_or_default()[0] - other.unwrap_or(0.);
                        // CSS 2 §10.3.3: auto margins of a box wider than its containing block are 0, and the end
                        // margin takes the rest.
                        let rtl =
                            parent.and_then(|parent| parent.get(Prop::DIRECTION)) == Some("rtl");
                        let used = match other {
                            _ if free < 0. => {
                                if left == rtl {
                                    free
                                } else {
                                    0.
                                }
                            }
                            Some(_) => free,
                            None => free / 2.,
                        };
                        format!(
                            "{}px",
                            css::value::tokens::number(f64::from((used * 64.).round() / 64.))
                        )
                    }
                    ("margin-top" | "margin-bottom", "auto") if size.is_some() => "0px".to_owned(),
                    // Layout gives a pseudo-element box its padding, so its sizes are not resolved again.
                    (
                        "width" | "height" | "padding-top" | "padding-right" | "padding-bottom"
                        | "padding-left",
                        _,
                    ) if let Some(rect) = pseudo_box => {
                        let borders = [
                            Prop::BORDER_TOP_WIDTH,
                            Prop::BORDER_RIGHT_WIDTH,
                            Prop::BORDER_BOTTOM_WIDTH,
                            Prop::BORDER_LEFT_WIDTH,
                        ]
                        .map(|prop| {
                            get(prop.name())
                                .strip_suffix("px")
                                .and_then(|px| px.parse::<f32>().ok())
                                .unwrap_or(0.)
                        });
                        let used = match &*name {
                            "width" | "height" => {
                                let axis = usize::from(&*name == "height");
                                match style.get(Prop::BOX_SIZING) == Some("border-box") {
                                    true => rect[2 + axis],
                                    false => rect[2 + axis] - rect[5 - axis] - rect[7 - axis],
                                }
                            }
                            side => {
                                let side = [
                                    "padding-top",
                                    "padding-right",
                                    "padding-bottom",
                                    "padding-left",
                                ]
                                .iter()
                                .position(|name| *name == side)
                                .unwrap_or(0);
                                rect[4 + side] - borders[side]
                            }
                        };
                        format!(
                            "{}px",
                            css::value::tokens::number(f64::from(
                                (used.max(0.) * 64.).round() / 64.
                            ))
                        )
                    }
                    // The image layers set the number of layers of the other lists.
                    (
                        "background-position"
                        | "background-size"
                        | "background-repeat"
                        | "background-attachment"
                        | "background-origin"
                        | "background-clip"
                        | "background-blend-mode"
                        | "mask-position"
                        | "mask-size"
                        | "mask-repeat"
                        | "mask-origin"
                        | "mask-clip"
                        | "mask-composite"
                        | "mask-mode",
                        _,
                    ) => {
                        let image = if name.starts_with("mask") {
                            "mask-image"
                        } else {
                            "background-image"
                        };
                        let count = css::value::tokens::components(&get(image), ',').len();
                        let layers = css::value::tokens::components(&value, ',');
                        if layers.len() == count {
                            return value;
                        }
                        (0..count)
                            .map(|index| layers[index % layers.len()].trim())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                    // Like Chrome, a rendered grid gives its used tracks, with the line names of explicit ones.
                    ("grid-template-columns" | "grid-template-rows", _)
                        if size.is_some()
                            && value != "subgrid"
                            && style
                                .get(Prop::DISPLAY)
                                .is_some_and(|display| display.ends_with("grid")) =>
                    {
                        let Some(tracks) = state.layout.tracks.get(&id) else {
                            return value;
                        };
                        let used = &tracks[usize::from(name == "grid-template-rows")];
                        let mut used = used.iter().map(|&track| {
                            format!(
                                "{}px",
                                css::value::tokens::number(f64::from((track * 64.).round() / 64.))
                            )
                        });
                        // `repeat()` with a count gives its tracks and names that many times.
                        let mut parts = Vec::new();
                        for part in css::value::tokens::components(&value, ' ') {
                            let repeat = part
                                .strip_prefix("repeat(")
                                .and_then(|inner| inner.strip_suffix(')')?.split_once(','))
                                .and_then(|(count, body)| {
                                    Some((count.trim().parse::<usize>().ok()?, body.trim()))
                                });
                            match repeat {
                                Some((count, body)) => {
                                    for _ in 0..count.min(1000) {
                                        parts.extend(css::value::tokens::components(body, ' '));
                                    }
                                }
                                None => parts.push(part),
                            }
                        }
                        let mut out: Vec<String> = Vec::new();
                        if value != "none" && !parts.iter().any(|part| part.starts_with("repeat("))
                        {
                            for part in parts {
                                match (part.strip_prefix('['), out.last_mut()) {
                                    // Names of one line join, like `[b a]`.
                                    (Some(names), Some(last)) if last.ends_with(']') => {
                                        last.pop();
                                        last.push(' ');
                                        last.push_str(names);
                                    }
                                    (Some(_), _) => out.push(part.to_owned()),
                                    (None, _) => out.extend(used.next()),
                                }
                            }
                        }
                        out.extend(used);
                        match out.is_empty() {
                            true => value,
                            false => out.join(" "),
                        }
                    }
                    ("content", "normal") if matches!(pseudo, "before" | "after") => {
                        "none".to_owned()
                    }
                    // Chrome gives the strings and `attr()` values of generated content as one string.
                    ("content", _)
                        if !pseudo.is_empty() && value != "none" && value != "normal" =>
                    {
                        // Counters keep their declared form, which `counting` holds.
                        let value = match styles.counting.get(&id) {
                            Some(sides) if matches!(pseudo, "before" | "after") => sides
                                [usize::from(pseudo == "after")]
                            .as_deref()
                            .map_or(value, str::to_owned),
                            _ => value,
                        };
                        let (mut parts, mut text) = (Vec::new(), None::<String>);
                        let quote = |text: String| {
                            format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
                        };
                        for part in css::value::tokens::components(&value, ' ') {
                            let mut input = cssparser::Parser::new(part);
                            let mut strings = None::<String>;
                            while let Ok(string) = input.expect_string() {
                                strings.get_or_insert_default().push_str(string);
                            }
                            let string = strings.filter(|_| input.is_exhausted()).or_else(|| {
                                let name = part.strip_prefix("attr(")?.strip_suffix(')')?.trim();
                                Some(doc.nodes[id].attrs.get(name).cloned().unwrap_or_default())
                            });
                            match string {
                                Some(string) => text.get_or_insert_default().push_str(&string),
                                None => {
                                    parts.extend(text.take().map(quote));
                                    parts.push(part.to_owned());
                                }
                            }
                        }
                        parts.extend(text.map(quote));
                        parts.join(" ")
                    }
                    _ => value,
                }
            };
            Ok(match args[1].as_str() {
                Some(name @ ("inset" | "margin")) => {
                    let sides = if name == "inset" {
                        ["top", "right", "bottom", "left"]
                    } else {
                        ["margin-top", "margin-right", "margin-bottom", "margin-left"]
                    }
                    .map(value);
                    json!(css::value::specified::shortest(
                        sides.each_ref().map(String::as_str)
                    ))
                }
                Some(name) => json!(value(name)),
                None => json!(
                    css::value::grammar::computed_names()
                        .chain(style.vars().iter().map(|(prop, _)| prop.name()))
                        .collect::<Vec<_>>()
                ),
            })
        }
        "dom.css" => {
            if doc.nodes[id].tag == "link" && doc.nodes[id].stylesheet.is_none() {
                return Ok(Value::Null);
            }
            let sheet = state.styles.cache.sheet(doc, id, &mut state.styles.sheets);
            Ok(sheet.map_or_else(|| json!([]), |sheet| sheet.cssom()))
        }
        "dom.sheet" => {
            let kind = args[1].as_str().unwrap_or_default();
            let text = args[2].as_str().unwrap_or_default();
            let (path, top) = path(kind, &args[3]);
            let result = state.styles.cache.edit_sheet(
                doc,
                id,
                &mut state.styles.sheets,
                top,
                |sheet, disabled| {
                    if kind == "disabled" {
                        *disabled = args[2].as_bool().unwrap_or(false);
                        return Ok(Value::Null);
                    }
                    sheet.edit(&path, edit(kind, text))
                },
            );
            state.styles.invalidate();
            state.dirty = true;
            Ok(result.unwrap_or_else(|error| json!({ "error": error })))
        }
        "dom.selection" => {
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
        "dom.userSelect" | "dom.selectionText" => {
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
            while !std::iter::successors(Some(last), |&id| doc.nodes[id].parent)
                .any(|id| id == root)
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
        // The renderer doesn't snap, so a box with no scroll offset, and none to come, is at 0 after any layout.
        "dom.scrolled" => Ok(json!(state.geometry.scroll.get(&id).is_some_and(
            |handle| {
                handle.offset() != renderer::Point::default()
                    || handle.target() != renderer::Point::default()
            }
        ))),
        "dom.scroll" => {
            let page = doc.nodes[id].parent == Some(0)
                && doc.nodes[id].tag == "html"
                && !state.geometry.scroll.contains_key(&id);
            // As in Chrome, a box that can't scroll keeps its offset at 0.
            if !page && !state.geometry.scroll.contains_key(&id) {
                let computed = state.restyle(doc, viewport);
                let style = computed.values.get(id);
                let scrolls = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                    .into_iter()
                    .any(|prop| {
                        style
                            .and_then(|style| style.get(prop))
                            .is_some_and(|value| !matches!(value, "visible" | "clip"))
                    });
                let boxed = style
                    .and_then(|style| style.get(Prop::DISPLAY))
                    .is_some_and(|value| !matches!(value, "inline" | "contents" | "none"));
                if doc.nodes[id].tag.starts_with('#') || !scrolls || !boxed {
                    return Ok(json!([0., 0.]));
                }
            }
            let handle = state
                .geometry
                .scroll
                .entry(if page { 0 } else { id })
                .or_default()
                .clone();
            let previous = handle.offset();
            let ratio = state.pixel_ratio.max(1.);
            let axis = |value: &Value, current: renderer::Pixels| {
                renderer::px(
                    value
                        .as_f64()
                        .map_or(f32::from(current), |v| -(v as f32 * ratio).round() / ratio),
                )
            };
            let target = geometry::scroll_offset(
                &handle,
                renderer::point(axis(&args[1], previous.x), axis(&args[2], previous.y)),
            );
            if target != handle.target() {
                let smooth = handle.extent() != renderer::Point::default()
                    && match args[3].as_str() {
                        Some("smooth") => true,
                        Some("instant") => false,
                        _ => {
                            state.restyle(doc, viewport).values[id].get(Prop::SCROLL_BEHAVIOR)
                                == Some("smooth")
                        }
                    };
                if smooth {
                    handle.animate_to(target, false);
                } else {
                    handle.set_offset(target);
                }
            }
            state.dirty |= target != previous;
            state.geometry.sync_scroll();
            Ok(json!([-f32::from(target.x), -f32::from(target.y)]))
        }
        "dom.canvas" => {
            if let Some(ops) = crate::jsc::payload::slice(&args[1]) {
                let ops: Vec<f64> = ops?
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|bytes| f64::from_ne_bytes(*bytes))
                    .collect();
                let data = args
                    .as_array()
                    .into_iter()
                    .flatten()
                    .skip(3)
                    .map(|value| {
                        crate::jsc::payload::slice(value).context("Expected image data")?
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut canvas = match state.media.canvases.remove(&id) {
                    Some(canvas) => canvas,
                    None => {
                        let canvas = new_canvas(&doc.nodes[id])?;
                        state.tree.note_created(canvas.pixels.data().len() >> 16);
                        canvas
                    }
                };
                let result = canvas.draw(
                    &ops,
                    args[2].as_str().unwrap_or(""),
                    &data,
                    &|(source, size)| match source.checked_sub(SNAPSHOT) {
                        Some(element) => state.media.snapshots.get(&element).map(|(pixels, _)| {
                            crate::render::canvas::Source::Shared(pixels.clone())
                        }),
                        None => state.media.source_pixels(source, Some(size)),
                    },
                    state
                        .fonts
                        .system
                        .as_deref()
                        .map(|system| (system, &state.fonts.aliases)),
                );
                state.media.canvases.insert(id, canvas);
                state.media.retire(id, result?);
            } else {
                state.media.retired.extend(
                    state
                        .media
                        .canvases
                        .remove(&id)
                        .filter(|canvas| canvas.present)
                        .map(|canvas| canvas.image),
                );
                state.media.retired.extend(state.media.images.remove(&id));
                state.media.sources.get_mut().remove(&id);
            }
            // Offscreen drawing changes nothing on the page, so it must not cost page frames.
            state.dirty |= doc.nodes[id].tag == "canvas";
            Ok(Value::Null)
        }
        "dom.canvasURL" => {
            use base64::Engine as _;
            let base64 = base64::engine::general_purpose::STANDARD;
            let png = match state.media.source_pixels(id, None) {
                Some(pixels) => crate::render::image::encode_png(&pixels)?,
                None => crate::render::image::encode_png(&new_canvas(&doc.nodes[id])?.pixels)?,
            };
            let (bytes, mime) = match args[2].as_str() {
                #[cfg(target_os = "macos")]
                Some("image/jpeg") => {
                    let quality = args[3].as_f64().filter(|q| (0. ..=1.).contains(q));
                    let jpeg = crate::native::native_call(
                        "image",
                        &json!({"op": "jpeg", "data": base64.encode(&png), "quality": quality.unwrap_or(0.92) * 100.}),
                    )?;
                    (
                        base64.decode(jpeg["png"].as_str().unwrap_or_default())?,
                        "image/jpeg",
                    )
                }
                _ => (png, "image/png"),
            };
            Ok(if args[1] == true {
                json!([crate::jsc::payload::write(bytes), mime])
            } else {
                json!(format!("data:{mime};base64,{}", base64.encode(bytes)))
            })
        }
        "dom.canvasPixels" => {
            let pixels = match state.media.source_pixels(id, None) {
                Some(pixels) => pixels,
                None => crate::render::canvas::Source::Shared(Rc::new(
                    new_canvas(&doc.nodes[id])?.pixels,
                )),
            };
            let x = args[1].as_i64().unwrap_or(0);
            let y = args[2].as_i64().unwrap_or(0);
            let width = args[3].as_u64().context("Expected width")? as usize;
            let height = args[4].as_u64().context("Expected height")? as usize;
            ensure!(
                width > 0
                    && height > 0
                    && width <= 16384
                    && height <= 16384
                    && width * height <= 16_777_216,
                "Invalid pixel region"
            );
            let mut bytes = vec![0u8; width * height * 4];
            let (w, h) = (i64::from(pixels.width()), i64::from(pixels.height()));
            let [left, right] = [x, x.saturating_add(width as i64)].map(|edge| edge.clamp(0, w));
            for sy in (y.max(0)..y.saturating_add(height as i64).min(h)).filter(|_| left < right) {
                let from = (sy * w + left) as usize..(sy * w + right) as usize;
                let at = (((sy - y) * width as i64 + left - x) * 4) as usize;
                let out = &mut bytes[at..at + from.len() * 4];
                if args[5] == true {
                    out.copy_from_slice(&pixels.data()[from.start * 4..from.end * 4]);
                    continue;
                }
                for (out, pixel) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(&pixels.pixels()[from])
                {
                    let color = pixel.demultiply();
                    *out = [color.red(), color.green(), color.blue(), color.alpha()];
                }
            }
            Ok(crate::jsc::payload::write(bytes))
        }
        "dom.canvasTake" => {
            let to = args[1].as_u64().context("Expected node id")? as usize;
            ensure!(to < doc.nodes.len(), "Invalid node id");
            let mut taken = match state.media.canvases.get_mut(&id) {
                Some(canvas) => {
                    let mut taken = crate::render::canvas::Canvas::new(
                        canvas.pixels.width(),
                        canvas.pixels.height(),
                    )?;
                    std::mem::swap(&mut canvas.pixels, &mut taken.pixels);
                    taken
                }
                None => {
                    let mut taken = new_canvas(&doc.nodes[id])?;
                    if let Some(pixels) = state.media.source_pixels(id, None) {
                        taken.pixels = (*pixels).clone();
                    }
                    taken
                }
            };
            taken.present = doc.nodes[to].tag == "canvas";
            if taken.present {
                taken.image = crate::render::canvas::render_image(&taken.pixels, Vec::new());
            }
            state.tree.note_created(taken.pixels.data().len() >> 16);
            state.dirty |= taken.present;
            let old = state.media.canvases.insert(to, taken);
            state
                .media
                .retire(to, old.filter(|old| old.present).map(|old| old.image));
            Ok(Value::Null)
        }
        "dom.canvasAdopt" => {
            let mut canvas = new_canvas(&doc.nodes[id])?;
            canvas.pixels = tiny_skia::Pixmap::from_vec(
                crate::web::bytes(&args[1])?,
                tiny_skia::IntSize::from_wh(canvas.pixels.width(), canvas.pixels.height())
                    .context("Invalid canvas size")?,
            )
            .context("Invalid canvas pixels")?;
            state.tree.note_created(canvas.pixels.data().len() >> 16);
            state.media.canvases.insert(id, canvas);
            Ok(Value::Null)
        }
        // The size of the last drawing of a drawable element in CSS pixels. With a node, that canvas gets a copy.
        "dom.snapshot" => {
            let Some((pixels, scale)) = state.media.snapshots.get(&id).cloned() else {
                return Ok(Value::Null);
            };
            if let Some(to) = args[1].as_u64() {
                let mut canvas =
                    crate::render::canvas::Canvas::new(pixels.width(), pixels.height())?;
                canvas.pixels = (*pixels).clone();
                state.tree.note_created(canvas.pixels.data().len() >> 16);
                state.media.canvases.insert(to as usize, canvas);
            }
            Ok(json!([
                pixels.width() as f32 / scale,
                pixels.height() as f32 / scale,
                pixels.width(),
                pixels.height()
            ]))
        }
        // Where a canvas drew a drawable element: `get`, `clear`, or `set` with a matrix and a clip, each of
        // which stays as it is when null.
        "dom.drawn" => {
            let floats = |value: &Value| {
                let list = value.as_array()?.iter().map(|v| Some(v.as_f64()? as f32));
                list.collect::<Option<Vec<f32>>>()
            };
            match args[1].as_str() {
                Some("get") => Ok(json!(
                    state.geometry.drawn.get(&id).map(|drawn| drawn.matrix)
                )),
                Some("clear") => {
                    state.dirty |= state.geometry.drawn.remove(&id).is_some();
                    Ok(Value::Null)
                }
                _ => {
                    let matrix = floats(&args[2]).and_then(|list| <[f32; 16]>::try_from(list).ok());
                    let clip = floats(&args[3]).and_then(|list| <[f32; 4]>::try_from(list).ok());
                    let old = state.geometry.drawn.remove(&id);
                    if !(args[4] == true && old.is_some()) {
                        state.geometry.drawn_order += 1;
                    }
                    let new = Drawn {
                        matrix: matrix
                            .or(old.as_ref().map(|old| old.matrix))
                            .unwrap_or(crate::css::value::transform::IDENTITY),
                        clip: clip.or(old.as_ref().and_then(|old| old.clip)),
                        order: match &old {
                            Some(old) if args[4] == true => old.order,
                            _ => state.geometry.drawn_order,
                        },
                    };
                    // The boxes take the pointer at their new place after the next frame.
                    state.dirty |= old.is_none_or(|old| old.matrix != new.matrix);
                    state.geometry.drawn.insert(id, new);
                    Ok(Value::Null)
                }
            }
        }
        "dom.canvasLink" => {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let token = NEXT.fetch_add(1, Ordering::Relaxed);
            let placeholder = state
                .media
                .placeholders
                .remove(&id)
                .unwrap_or_else(|| Placeholder {
                    frames: state.media.frames.clone(),
                    node: id,
                    wake: crate::jsc::Waker::current(),
                });
            PLACEHOLDERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(token, placeholder);
            Ok(json!(token))
        }
        "dom.canvasBind" => {
            let token = args[1].as_u64().context("Expected a token")?;
            let placeholder = PLACEHOLDERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&token)
                .context("The canvas was already transferred")?;
            state.media.placeholders.insert(id, placeholder);
            Ok(Value::Null)
        }
        "dom.image" => {
            let bytes = crate::web::bytes(&args[1])?;
            Ok(state.media.load_image(id, move || Ok(bytes)))
        }
        "dom.font" => {
            let family = args[1].as_str().context("Expected font family")?;
            Ok(match args[2].as_str() {
                Some(name) => json!(crate::render::fonts::local_font(state, family, name)),
                None => json!(crate::render::fonts::add_font(
                    state,
                    family,
                    crate::web::bytes(&args[2])?
                )?),
            })
        }
        "dom.canvasFont" => Ok(json!(crate::render::canvas::font(
            args[1].as_str().unwrap_or("")
        ))),
        // As a canvas serializes a color: `#rrggbb`, or `rgba()` with the shortest alpha that rounds back.
        "dom.canvasColor" => Ok(
            match crate::css::value::color::color(args[1].as_str().unwrap_or("")) {
                Some((rgb, 255)) => json!(format!("#{:06x}", rgb & 0xff_ffff)),
                Some((rgb, alpha)) => {
                    let [_, r, g, b] = rgb.to_be_bytes();
                    let value = (1..=3)
                        .map(|digits| format!("{:.*}", digits, f64::from(alpha) / 255.))
                        .find(|text| {
                            text.parse::<f64>()
                                .is_ok_and(|v| (v * 255.).round() as u8 == alpha)
                        })
                        .unwrap_or_else(|| format!("{:.3}", f64::from(alpha) / 255.));
                    let value = value.trim_end_matches('0').trim_end_matches('.');
                    json!(format!("rgba({r}, {g}, {b}, {value})"))
                }
                None => Value::Null,
            },
        ),
        "dom.fontSources" => Ok(json!(crate::render::fonts::sources(
            args[1].as_str().context("Expected font sources")?
        ))),
        "dom.fontCheck" => {
            let family = args[1].as_str().context("Expected font family")?;
            let family = state
                .fonts
                .aliases
                .get(family)
                .map(String::as_str)
                .unwrap_or(family);
            Ok(json!(
                matches!(family, "serif" | "sans-serif" | "monospace" | "system-ui")
                    || crate::render::canvas::fonts(true).0.faces().any(|face| face
                        .families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(family)))
            ))
        }
        "dom.textOffset" | "dom.textPoint" => {
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
        "dom.svgBox" => {
            let boxes =
                std::iter::successors(Some(id), |&node| doc.nodes[node].parent).find_map(|node| {
                    state
                        .rasters
                        .svg_boxes
                        .get(&node)
                        .map(|boxes| (node, boxes))
                });
            Ok(json!(
                boxes
                    .and_then(|(root, (_, _, _, boxes, content, ..))| if root == id {
                        Some(*content)
                    } else {
                        boxes
                            .iter()
                            .find(|(node, ..)| *node == id)
                            .map(|(.., object)| *object)
                    })
                    .unwrap_or_default()
            ))
        }
        "dom.fragments" => Ok(state
            .geometry
            .fragments
            .get(&id)
            .map_or(Value::Null, |parts| json!(parts))),
        "dom.textRect" | "dom.textIndex" => {
            let password = doc.nodes[id]
                .password()
                .then(|| doc.nodes[id].current_value().unwrap_or("").to_owned());
            let shown = |offset| {
                password
                    .as_deref()
                    .map_or(offset, |value| text::bullets(value, offset))
            };
            let real = |index| {
                password
                    .as_deref()
                    .map_or(index, |value| text::unbullet(value, index))
            };
            let start = shown(args[1].as_u64().unwrap_or(0) as usize);
            let end = args[2].as_u64().map_or(start, |end| shown(end as usize));
            if let Some((layout, text, range, rows, font)) = state.layout.text_layouts.get(&id) {
                let content = &text[range.clone()];
                let source = doc.nodes[id].text.as_str();
                let [dx, dy] = state.geometry.scroll_shift(&state.styles.walk, id);
                if op == "dom.textIndex" {
                    let position = renderer::point(
                        renderer::px(args[1].as_f64().unwrap_or(0.) as f32 - dx),
                        renderer::px(args[2].as_f64().unwrap_or(0.) as f32 - dy),
                    );
                    let index = crate::render::text::text_index(layout, text, rows, position, true)
                        .unwrap_or_else(|index| index)
                        .clamp(range.start, range.end);
                    let offset = text::source_offset(source, content, index - range.start);
                    return Ok(json!(real(offset)));
                }
                let mut rects = crate::render::text::text_rects(
                    layout,
                    text,
                    rows,
                    [start, end]
                        .map(|index| range.start + text::shown_index(source, content, index)),
                    *font,
                );
                rects
                    .iter_mut()
                    .for_each(|rect| (rect[0], rect[1]) = (rect[0] + dx, rect[1] + dy));
                return Ok(json!(rects));
            }
            // Text that this frame did not lay out, like the text of a block far from the view, lays out with its
            // block on the next reflow: `null` asks the script for one, as `dom.skipped` does.
            let forced = &mut state.layout.measurements.forced;
            if doc.nodes[id].parent.is_some() && forced.insert(id) {
                state.layout.measurements.waking.push(id);
                state.layout.cull.reskipped = true;
                return Ok(Value::Null);
            }
            Ok(if op == "dom.textRect" {
                json!([])
            } else {
                json!(0)
            })
        }
        "dom.measureText" => {
            let system = state
                .fonts
                .system
                .as_ref()
                .context("Text measurement requires a displayed window")?;
            let (line, [ascent, descent, em_ascent, em_descent]) = crate::render::canvas::line(
                system,
                &state.fonts.aliases,
                args[2].as_str().unwrap_or("10px sans-serif"),
                args[1].as_str().context("Expected text")?,
                [
                    args[3].as_f64().unwrap_or(0.) as f32,
                    args[4].as_f64().unwrap_or(0.) as f32,
                ],
            );
            let [left, right, top, bottom] = crate::render::canvas::ink(system, &line);
            Ok(json!([
                f32::from(line.width),
                left,
                right,
                top,
                bottom,
                ascent,
                descent,
                em_ascent,
                em_descent
            ]))
        }
        "dom.canvasQuery" => {
            let Some(canvas) = state.media.canvases.get(&id) else {
                return Ok(json!(if args[1].is_number() {
                    json!(false)
                } else {
                    json!([1, 0, 0, 1, 0, 0])
                }));
            };
            Ok(match args[1].as_f64() {
                Some(x) => json!(canvas.hit(
                    x as f32,
                    args[2].as_f64().unwrap_or(0.) as f32,
                    args[3].as_bool().unwrap_or(false),
                    args[4].as_bool().unwrap_or(false),
                    args[5].as_u64(),
                )),
                None => json!(canvas.transform()),
            })
        }
        "dom.client" => {
            let rect = state.geometry.mirror.get(&id).copied().unwrap_or_default();
            // Reuse the last frame's styles for resize observers to avoid a style pass per callback.
            let last = (args[1].as_bool() == Some(true))
                .then(|| {
                    state
                        .styles
                        .computed
                        .clone()
                        .or_else(|| state.styles.previous.clone())
                })
                .flatten()
                .filter(|last| id < last.values.len() && last.lengths.get(id).is_some());
            let computed = match last {
                Some(last) => last,
                None => state.restyle(doc, viewport),
            };
            let (style, units) = (&computed.values[id], &computed.lengths[id]);
            let tag = doc.nodes[id].tag.as_str();
            if (style::ROW..=style::FOOTER).contains(&style::role(&computed, id)) {
                return Ok(json!([
                    0., 0., rect[2], rect[3], 0., 0., 0., 0., rect[2], rect[3], 0., 0., 0.
                ]));
            }
            if tag.starts_with('#')
                || style
                    .get(Prop::DISPLAY)
                    .is_none_or(|value| matches!(value, "inline" | "contents"))
            {
                // The last value says that the box is inline, so it has no client box.
                return Ok(json!([
                    0., 0., 0., 0., 0., 0., 0., 0., rect[2], rect[3], 0., 0., 1.
                ]));
            }
            let shared = style::role(&computed, id) == style::CELL
                && std::iter::successors(doc.nodes[id].parent, |&parent| doc.nodes[parent].parent)
                    .find(|&parent| style::table_box(style::role(&computed, parent)))
                    .is_some_and(|table| {
                        computed
                            .values
                            .get(table)
                            .and_then(|style| style.get(Prop::BORDER_COLLAPSE))
                            == Some("collapse")
                    });
            let base = doc.nodes[id]
                .parent
                .and_then(|parent| state.geometry.mirror.get(&parent))
                .map_or(viewport[0], |rect| rect[2]);
            // The bounds are those of the last frame, which laid out the animated padding and borders.
            let now = state.styles.animating.get(&id).unwrap_or(style);
            let borders = units
                .borders(now, base)
                .map(|width| width / if shared { 2. } else { 1. });
            let padding = units
                .edges(now, css::properties::PADDING, base)
                .map(|value| value.max(0.));
            let bars = state
                .geometry
                .scroll
                .get(&id)
                .and_then(|handle| {
                    let look = crate::render::scrollbar::Look::new(
                        style,
                        computed.scrollbars.get(&id).map(|parts| &**parts),
                        units,
                        None,
                    )?;
                    let scrolls = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y].map(|prop| {
                        style
                            .get(prop)
                            .is_some_and(|value| matches!(value, "auto" | "scroll"))
                    });
                    Some(look.space(look.shown(scrolls, handle.extent())))
                })
                .unwrap_or_default();
            let gutters = if style
                .get(Prop::SCROLLBAR_GUTTER)
                .is_some_and(|value| value.contains("both-edges"))
            {
                2.
            } else {
                1.
            };
            // Like offsetWidth, client sizes ignore transforms, so a transformed box takes its layout size,
            // and the shift from its bounds to its own untransformed corner.
            let transformed = state.geometry.mirror.is_transformed(id);
            let size = state
                .layout
                .cull
                .widths
                .get(&id)
                .filter(|_| transformed)
                .map_or([rect[2], rect[3]], |inner| {
                    [
                        inner[0] + padding[1] + padding[3] + borders[1] + borders[3],
                        inner[1] + padding[0] + padding[2] + borders[0] + borders[2],
                    ]
                });
            let local = renderer::Rect::new(
                renderer::point(renderer::px(0.), renderer::px(0.)),
                renderer::size(renderer::px(size[0]), renderer::px(size[1])),
            );
            // The bounds are those of the last frame, so its animated transform moved them.
            let shift = match transformed
                .then(|| crate::css::value::transform::transform(now, units, local, None))
                .flatten()
            {
                Some(m) if m[3] == 0. && m[7] == 0. && m[15] == 1. => {
                    let [x, y] = [size[0] / 2., size[1] / 2.];
                    [
                        rect[2] / 2. - m[0] * x - m[4] * y - m[12],
                        rect[3] / 2. - m[1] * x - m[5] * y - m[13],
                    ]
                }
                _ => [0., 0.],
            };
            Ok(json!([
                borders[3],
                borders[0],
                (size[0] - borders[1] - borders[3] - bars[1] * gutters).max(0.),
                (size[1] - borders[0] - borders[2] - bars[0]).max(0.),
                padding[0],
                padding[1],
                padding[2],
                padding[3],
                size[0],
                size[1],
                shift[0],
                shift[1],
                0.
            ]))
        }
        "dom.innerText" => {
            let computed = state.restyle(doc, viewport);
            Ok(json!(rendered_text(doc, state, &computed, id, None)))
        }
        "dom.caretText" => {
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
        // The boxes around the node that skip their contents: 2 if one has `content-visibility: hidden`, 1 if
        // all have `auto`, 0 if none does, and -1 if the node or a box around it has `display: none`. With `true`,
        // they show their contents until the next frame, so a layout gives the geometry of the node.
        "dom.skipped" => {
            let computed = state.restyle(doc, viewport);
            let (mut kind, mut roots) = (0, Vec::new());
            // An `auto` box that shows its contents from the next frame on still skips them.
            let hidden = state.layout.cull.visible.get(id) == Some(&false);
            let mut node = Some(id);
            while let Some(at) = node {
                let style = &computed.values[at];
                if style.get(Prop::DISPLAY) == Some("none") {
                    return Ok(json!(-1));
                }
                if at != id
                    && (crate::render::skipping(doc, state, &computed, at, false)
                        || hidden
                            && style.get(Prop::CONTENT_VISIBILITY) == Some("auto")
                            && state.layout.measurements.shown.contains_key(&at))
                {
                    roots.push(at);
                    kind = kind.max(if style.get(Prop::CONTENT_VISIBILITY) == Some("hidden") {
                        2
                    } else {
                        1
                    });
                }
                node = state.tree.flat_parent(doc, &computed, at);
            }
            if args[1].as_bool() == Some(true) {
                let measured = &mut state.layout.measurements;
                for root in roots {
                    if measured.forced.insert(root) {
                        measured.waking.push(root);
                        state.layout.cull.reskipped = true;
                    }
                }
            }
            Ok(json!(kind))
        }
        _ => anyhow::bail!("Unknown DOM operation: {op}"),
    }
}

fn tree(doc: &Document, root: usize, inclusive: bool) -> Value {
    let mut list = Vec::with_capacity(1024);
    let mut pending: Vec<_> = if inclusive {
        vec![root]
    } else {
        doc.nodes[root].children.iter().rev().copied().collect()
    };
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        let lazy = node.stylesheet.is_some() && node.tag == "#text"
            || !node.text.is_empty()
                && node.parent.is_some_and(|parent| {
                    matches!(doc.nodes[parent].tag.as_str(), "script" | "style")
                });
        let text = (!lazy).then_some(&node.text);
        let count = node.attrs.iter().count();
        let at = list.len();
        let _ = serde_json::to_writer(&mut list, &(id, node.parent, &node.tag, text, count));
        if at > 0 {
            list[at] = b',';
        }
        list.pop();
        for pair in &node.attrs {
            let at = list.len();
            let _ = serde_json::to_writer(&mut list, &pair);
            list[at] = b',';
            list.pop();
        }
        pending.extend(node.children.iter().rev());
    }
    if list.is_empty() {
        list.push(b'[');
    }
    list.push(b']');
    Value::String(String::from_utf8(list).unwrap_or_default())
}

pub(crate) fn layout_flags(doc: &mut Document, state: &mut PageState, viewport: [f32; 2]) {
    let computed = state.restyle(doc, viewport);
    if state.geometry.layout_epoch == state.styles.epoch {
        return;
    }
    state.geometry.layout_epoch = state.styles.epoch;
    let mut known = rustc_hash::FxHashMap::default();
    for (id, style) in computed.values.iter().enumerate() {
        let bits = *known.entry(Rc::as_ptr(style)).or_insert_with(|| {
            let clip = [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
                .into_iter()
                .any(|prop| {
                    style
                        .get(prop)
                        .is_some_and(|value| matches!(value, "hidden" | "clip" | "auto" | "scroll"))
                });
            let position = style.get(Prop::POSITION).unwrap_or("static");
            let positioned = crate::render::effects::establishes(style).absolute;
            [clip, positioned, position == "fixed"]
        });
        state.geometry.mirror.style_flags(id, bits);
    }
}

fn display<'a>(computed: &style::ComputedStyles, id: usize, value: &'a str) -> &'a str {
    match style::role(computed, id) {
        0 => value,
        role => style::TABLE_DISPLAYS[role as usize],
    }
}

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
                    _ if caret.found >= 0 || caret.waiting => (0, budget),
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
                    caret.space.get_or_insert((node as i64, offset as i64));
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
            caret.hidden = [caret.text.is_empty() || caret.hidden[0], true];
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
    caret.reach_all();
    json!([
        String::from_utf16_lossy(&caret.text),
        caret.runs,
        caret.found,
        start,
        end,
        caret.hidden
    ])
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
    // Open entries carry whether the parent blockifies its children, close entries the required line breaks
    // and the table state to restore.
    let mut pending = vec![(root, None, false, false)];
    while let Some((node, close, a, b)) = pending.pop() {
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
        let table = style::table_box(style::role(computed, node));
        if let Some(required) = close {
            if table {
                (rows, cells) = (a, b);
            }
            breaks = breaks.max(required);
            continue;
        }
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
                    text::utf16_index(value, start),
                    text::utf16_index(value, end),
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
        } else if a
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
        pending.push((node, Some(required), rows, cells));
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
        if crate::render::skipping(doc, state, computed, node, false) {
            continue;
        }
        let blockify =
            display.ends_with("flex") || display.ends_with("grid") || display == "contents" && a;
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
                .map(|&child| (child, None, blockify, false)),
        );
    }
    text
}

fn path(kind: &str, value: &Value) -> (Vec<usize>, Option<(usize, bool)>) {
    let path: Vec<usize> = value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|item| Some(item.as_u64()? as usize))
                .collect()
        })
        .unwrap_or_default();
    let top = (matches!(kind, "insert" | "delete") && path.len() == 1)
        .then(|| (path[0], kind == "insert"));
    (path, top)
}

fn edit<'a>(kind: &str, text: &'a str) -> css::sheet::Edit<'a> {
    match kind {
        "insert" => css::sheet::Edit::Insert(text),
        "style" => css::sheet::Edit::Declarations(text),
        "selector" => css::sheet::Edit::Selector(text),
        "keyframe" => css::sheet::Edit::Keyframe(text),
        "replace" => css::sheet::Edit::Replace(text),
        _ => css::sheet::Edit::Delete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::context::JsContext,
        host::{Host, Page},
        jsc::JavaScriptCore,
        jsc::Role,
    };
    use std::cell::RefCell;

    fn engine(html: &str) -> (Rc<RefCell<Host>>, JsContext) {
        let host = Rc::new(RefCell::new(Host::default()));
        let page = Page {
            doc: Document::parse(html),
            dirty: false,
            ..Default::default()
        };
        host.borrow_mut().pages.insert(1, page);
        let engine = JsContext::new(
            JavaScriptCore::load().unwrap(),
            host.clone(),
            Role::Renderer(1),
        )
        .unwrap();
        (host, engine)
    }

    // `Walk::update` compares each patched walk with a fresh one in debug builds.
    #[test]
    fn patched_walks_match_fresh_walks() {
        let mut doc = super::Document::parse(&format!(
            "<body>{}</body>",
            "<ul><li><b>a</b><i>b</i></li><li>c<span><em>d</em></span></li></ul>".repeat(40)
        ));
        let mut seed = 7u64;
        let mut random = |below: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % below
        };
        let mut composed = rustc_hash::FxHashMap::default();
        let mut walk = super::Walk::default();
        walk.update(&doc, &composed, None);
        let mut patches = 0;
        for _ in 0..600 {
            for _ in 0..1 + random(3) {
                let (a, b) = (1 + random(doc.nodes.len() - 1), random(doc.nodes.len()));
                match random(6) {
                    0 => doc.nodes.push(super::Node {
                        tag: "p".into(),
                        ..Default::default()
                    }),
                    1 if doc.nodes[a].children.is_empty() => {
                        if let Some(parent) = doc.nodes[a].parent.take() {
                            doc.nodes[parent].children.retain(|&child| child != a);
                        }
                    }
                    2 => doc.nodes[b].children.reverse(),
                    3 => {
                        composed.remove(&b);
                        let mut children = doc.nodes[b].children.clone();
                        let (turn, len) =
                            (random(children.len().max(1)), random(children.len() + 1));
                        children.rotate_left(turn);
                        children.truncate(len);
                        if random(2) == 0 {
                            composed.insert(b, children);
                        }
                    }
                    _ => {
                        let mut above = Some(b);
                        while above.is_some_and(|id| id != a) {
                            above = above.and_then(|id| doc.nodes[id].parent);
                        }
                        let mut root = b;
                        while let Some(parent) = doc.nodes[root].parent {
                            root = parent;
                        }
                        if above.is_some()
                            || doc.nodes[b].tag.starts_with('#')
                            || root != 0 && random(8) != 0
                        {
                            continue;
                        }
                        if let Some(parent) = doc.nodes[a].parent.take() {
                            doc.nodes[parent].children.retain(|&child| child != a);
                            composed.remove(&parent);
                        }
                        composed.remove(&b);
                        let at = random(doc.nodes[b].children.len() + 1);
                        doc.nodes[b].children.insert(at, a);
                        doc.nodes[a].parent = Some(b);
                    }
                }
            }
            // A composed list names only children of its node.
            composed.retain(|&id, children: &mut Vec<usize>| {
                children
                    .iter()
                    .all(|&child| doc.nodes[child].parent == Some(id))
            });
            let (old, serial) = (walk.snapshot(), walk.serial);
            walk.update(&doc, &composed, None);
            let Some(touched) = walk.since(serial) else {
                continue;
            };
            patches += 1;
            let untouched = |nodes: &[(u32, u32)]| {
                let kept = |node: &&(u32, u32)| !touched.contains(&node.0);
                nodes.iter().filter(kept).copied().collect::<Vec<_>>()
            };
            assert_eq!(untouched(&old), untouched(walk.order()));
            assert!(
                walk.order()
                    .iter()
                    .all(|&(id, _)| walk.contains(id as usize))
            );
        }
        assert!(
            patches > 300 && walk.order().len() > 300,
            "{patches} {}",
            walk.order().len()
        );
    }

    #[test]
    fn offsets_map_through_collapsed_white_space() {
        let (source, shown) = ("  a  b\n c ", "a b c");
        for (offset, byte) in [
            (0, 0),
            (2, 0),
            (3, 1),
            (5, 2),
            (6, 3),
            (8, 4),
            (9, 5),
            (10, 5),
        ] {
            assert_eq!(
                super::text::shown_index(source, shown, offset),
                byte,
                "{offset}"
            );
        }
        assert_eq!(super::text::source_offset(source, shown, 0), 0);
        assert_eq!(super::text::source_offset(source, shown, 1), 3);
        assert_eq!(super::text::source_offset(source, shown, 2), 4);
        assert_eq!(super::text::source_offset(source, shown, 3), 6);
        assert_eq!(super::text::shown_index("héllo", "héllo", 2), 3);
        assert_eq!(super::text::shown_index("", "abc", 2), 2);
        // `text-transform: uppercase` makes one `ß` two letters.
        let (source, shown) = ("groß ist", "GROSS IST");
        assert_eq!(super::text::shown_index(source, shown, 4), 5);
        assert_eq!(super::text::shown_index(source, shown, 5), 6);
        assert_eq!(super::text::source_offset(source, shown, 5), 4);
        assert_eq!(super::text::source_offset(source, shown, 4), 3);
        assert_eq!(super::text::source_offset(source, shown, 7), 6);
    }

    #[test]
    fn kept_containers_keep_their_sizes_between_frames() {
        let mut state = crate::render::PageState::default();
        state.layout.containers.insert(1, [150., 20.]);
        state.update_containers(&Document::default());
        assert_eq!(state.layout.containers.get(&1), Some(&[150., 20.]));
    }

    #[test]
    fn password_fields_show_a_bullet_per_code_point() {
        let doc = Document::parse("<input type=password><input>");
        let [password, text] = [1, 2].map(|nth| {
            doc.nodes
                .iter()
                .filter(|node| node.tag == "input")
                .nth(nth - 1)
                .unwrap()
        });
        let value = "a\u{1F600}b";
        assert_eq!(password.shown_text(value.into()), "\u{2022}".repeat(3));
        assert_eq!(text.shown_text(value.into()), value);
        assert_eq!(
            [0, 1, 2, 3, 4].map(|offset| text::bullets(value, offset)),
            [0, 1, 2, 2, 3]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|index| text::unbullet(value, index)),
            [0, 1, 3, 4]
        );
    }

    #[test]
    fn font_faces_load_woff_and_list_css_faces() {
        let fixtures = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fonts"));
        let (host, engine) = engine(
            r#"<style>@font-face{font-family:Css;src:url(test.woff) format("woff")}
                    @font-face{font-family:Later;src:url(later.woff);unicode-range:U+0400-04FF}</style><p>AB</p>"#,
        );
        std::sync::Arc::make_mut(&mut host.borrow_mut().app.assets)
            .insert("test.woff".into(), fixtures.join("test.woff"));
        {
            let mut host = host.borrow_mut();
            let state = host.state(1);
            state.fonts.system = Some(Rc::new(renderer::TextSystem::new()));
            state.base = url::Url::parse("apps:///index.html").ok();
        }
        let bytes = std::fs::read(fixtures.join("test-cff.woff2")).unwrap();
        let bytes: Vec<_> = bytes.iter().map(u8::to_string).collect();
        engine
            .eval(
                &format!(
                    r#"
            globalThis.log = [];
            const face = new FontFace('Bytes', new Uint8Array([{}]));
            document.fonts.add(face);
            face.load().then(() => log.push('bytes ' + face.status));
            new FontFace('Bad', 'url(a.eot) format("embedded-opentype")').load().catch(error => log.push('bad ' + error.name));
            new FontFace('Local', 'local("No Such Font 123")').load().catch(error => log.push('local ' + error.name));
            const junk = new FontFace('Junk', new Uint8Array([1, 2, 3, 4])); junk.loaded.catch(error => log.push('junk ' + (error instanceof DOMException) + ' ' + error.name + ' ' + junk.status));
            document.fonts.ready.then(fonts => {{
                log.push([...fonts].map(face => face.family + ' ' + face.status).join());
                const context = document.createElement('canvas').getContext('2d');
                for (const family of ['Bytes', 'Css']) {{ context.font = '10px ' + family; log.push(context.measureText('AB').width); }}
                log.push(document.fonts.check('10px Css'), document.fonts.check('10px Later'), document.fonts.check('10px Later', 'Ж'), document.fonts.has(face));
            }});
        "#,
                    bytes.join(",")
                ),
                "test",
            )
            .unwrap();
        for _ in 0..5 {
            engine.tick().unwrap();
        }
        assert_eq!(
            engine.eval("JSON.stringify(log)", "test").unwrap(),
            r#"["bytes loaded","bad SyntaxError","local NetworkError","junk true SyntaxError error","Css loaded,Later unloaded,Bytes loaded",15,15,true,true,false,true]"#
        );
    }

    #[test]
    fn detached_nodes_keep_styles_that_a_frame_holds() {
        let (host, engine) = engine("<html><body><div id='editor'></div></body></html>");
        engine
            .eval(
                "const editor=document.getElementById('editor');document.createElement('i');getComputedStyle(editor).color;",
                "test",
            )
            .unwrap();
        let held = host.borrow().pages[&1]
            .state
            .styles
            .computed
            .clone()
            .unwrap();
        engine
            .eval(
                "for(let i=0;i<20;i++)document.createElement('b').textContent='x';getComputedStyle(editor).color",
                "test",
            )
            .unwrap();
        assert!(Rc::ptr_eq(
            &held,
            host.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .unwrap()
        ));
    }
    #[test]
    fn detached_editor_overlays_preserve_connected_styles() {
        let (host, engine) =
            engine("<html><body><div id='editor' style='color:red'></div></body></html>");
        engine
            .eval(
                "const editor=document.getElementById('editor');getComputedStyle(editor).color;",
                "test",
            )
            .unwrap();
        let cached = Rc::as_ptr(
            host.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .unwrap(),
        );
        engine
            .eval(
                "editor.setAttribute('style','color:red');editor.removeAttribute('missing')",
                "test",
            )
            .unwrap();
        assert_eq!(
            cached,
            Rc::as_ptr(
                host.borrow().pages[&1]
                    .state
                    .styles
                    .computed
                    .as_ref()
                    .unwrap()
            )
        );
        engine
            .eval(
                r#"
            const fragment=document.createDocumentFragment();
            for(let i=0;i<100;i++) {
                const overlay=document.createElement('div');
                overlay.style.cssText='position:absolute;color:blue';
                overlay.dataset.selectionRange='';
                overlay.textContent='selection';
                fragment.append(overlay);
                if(getComputedStyle(editor).color!=='rgb(255, 0, 0)')throw Error('Connected style changed');
            }
        "#,
                "test",
            )
            .unwrap();
        assert_eq!(
            cached,
            Rc::as_ptr(
                host.borrow().pages[&1]
                    .state
                    .styles
                    .computed
                    .as_ref()
                    .unwrap()
            )
        );
        engine.eval(r#"
            editor.append(fragment);
            if(getComputedStyle(editor.firstChild).color!=='rgb(0, 0, 255)')throw Error('Inserted style was stale');
            fragment.append(editor.firstChild);
        "#, "test").unwrap();
        assert!(host.borrow().pages[&1].state.styles.computed.is_none());
        engine.eval(r#"
            const root=editor.attachShadow({mode:'open'});
            root.innerHTML='<span style="color:green">text</span>';
            if(getComputedStyle(root.firstChild).color!=='rgb(0, 128, 0)')throw Error('Shadow style');
            root.firstChild.style.color='purple';
            if(getComputedStyle(root.firstChild).color!=='rgb(128, 0, 128)')throw Error('Shadow mutation was stale');
        "#, "test").unwrap();
    }

    #[test]
    fn invalidation_skips_text_edits_and_unused_states() {
        let (host, engine) = engine(
            "<html><head><style id=s>p:empty{color:red}</style></head><body><p id=p>text</p><svg><text>a</text></svg></body></html>",
        );
        let computed = || {
            host.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .map(Rc::as_ptr)
        };
        let p = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.tag == "p")
            .unwrap();
        let revision = || host.borrow().pages[&1].doc.nodes.latest();
        let hover = |ids: &[usize]| {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
        };
        engine
            .eval(
                "const p=document.getElementById('p');getComputedStyle(p).color;",
                "test",
            )
            .unwrap();
        let cached = computed();
        assert!(cached.is_some());
        let built = revision();
        engine
            .eval(
                "p.firstChild.data='other';document.querySelector('svg text').firstChild.data='b';",
                "test",
            )
            .unwrap();
        hover(&[p]);
        hover(&[]);
        assert_eq!(computed(), cached);
        assert_ne!(revision(), built);
        engine
            .eval(
                r#"
            p.firstChild.data='';
            if(getComputedStyle(p).color!=='rgb(255, 0, 0)')throw Error('Empty text was stale');
            const sheet=document.getElementById('s');
            sheet.textContent='p:hover{color:blue}';
            sheet.firstChild.data='p:hover{color:green}';
            if(getComputedStyle(p).color==='rgb(0, 128, 0)')throw Error('Hover without state');
        "#,
                "test",
            )
            .unwrap();
        hover(&[p]);
        assert!(computed().is_none());
        engine
            .eval(
                "if(getComputedStyle(p).color!=='rgb(0, 128, 0)')throw Error('Hover state was stale')",
                "test",
            )
            .unwrap();
        host.borrow_mut()
            .pages
            .get_mut(&1)
            .map(|page| &mut page.state)
            .unwrap()
            .fonts
            .aliases
            .insert("Brand".into(), "Real Font".into());
        engine
            .eval("p.style.fontFamily='Brand';if(getComputedStyle(p).fontFamily!=='Real Font')throw Error('Alias')", "test")
            .unwrap();
        let first = host.borrow().pages[&1]
            .state
            .styles
            .computed
            .as_ref()
            .unwrap()
            .values[p]
            .clone();
        host.borrow_mut()
            .pages
            .get_mut(&1)
            .map(|page| &mut page.state)
            .unwrap()
            .styles
            .computed = None;
        engine.eval("getComputedStyle(p).color", "test").unwrap();
        assert!(Rc::ptr_eq(
            &first,
            &host.borrow().pages[&1]
                .state
                .styles
                .computed
                .as_ref()
                .unwrap()
                .values[p]
        ));
    }

    #[test]
    fn incremental_styles_match_full_passes() {
        let (host, engine) = engine("<html><head><style>
                    html{font-size:16px} .on{color:red} .dark .leaf{color:blue} .a+.b{color:green}
                    .box:has(.on){background-color:yellow} li:nth-child(2n){color:purple} p:empty{color:teal}
                    .rem{padding-left:2rem} .em{padding-top:2em} .vars{color:var(--c,gray)} .x::before{content:'a'}
                    .s::selection{color:white} .h:hover{color:orange} .none{display:none} [data-k=v]{color:navy}
                    </style></head><body><div id=root class=box><span id=leaf class=leaf>x</span>
                    <i class=a id=a></i><i class=b></i><ul id=list><li>1</li><li>2</li><li>3</li></ul>
                    <p id=p>t</p><div id=sized class=em><b class=rem>r</b><b class=em>e</b></div>
                    <div id=vars class=vars><span class=vars>v</span></div><div id=hover class='h s'>h</div>
                    <div id=k>k</div></div></body></html>");
        let hover = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "hover"))
            .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['root','leaf','a','list','p','sized','vars','hover','k'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "leaf.classList.add('on')",
            "root.classList.add('dark')",
            "a.classList.remove('a')",
            "a.classList.add('a')",
            "leaf.classList.remove('on')",
            "list.prepend(document.createElement('li'))",
            "list.firstChild.remove()",
            "p.firstChild.data=''",
            "document.documentElement.style.fontSize='20px'",
            "sized.style.fontSize='30px'",
            "vars.style.setProperty('--c','red')",
            "vars.firstChild.style.setProperty('--c','blue')",
            "leaf.classList.add('x')",
            "hover.classList.remove('s')",
            "hover.classList.add('none')",
            "hover.classList.remove('none')",
            "k.dataset.k='v'",
            "list.append(sized)",
            "root.prepend(list)",
            "root.removeChild(p)",
            "root.append(p)",
            "document.documentElement.style.fontSize=''",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        for ids in [vec![hover], vec![]] {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
            engine.eval(read, "test").unwrap();
        }
        #[cfg(debug_assertions)]
        assert!(host.borrow().pages[&1].state.styles.cache.incremental >= 20);
    }

    #[test]
    fn incremental_styles_follow_class_changes() {
        let (host, engine) = engine("<html><head><style>
                    .on{color:red} .dark .leaf{color:blue} .a+.b{color:green} .box:has(.hit){background-color:yellow}
                    li:nth-child(1 of .pick){color:purple} .wrap:not(.off) .inner{color:teal} .nest{& .deep{color:navy}}
                    .row.sel{color:olive} .row b{padding-left:1px}
                    </style></head><body><div id=root class=box><div id=wrap class=wrap><span id=inner class=inner>i</span></div>
                    <span id=leaf class=leaf>x</span><i id=a></i><i class=b></i><ul><li id=l1>1</li><li id=l2>2</li></ul>
                    <div id=nest><b class=deep>d</b></div>
                    <div id=row class=row><b>r</b></div><em id=hit>h</em></div></body></html>");
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['root','wrap','leaf','a','l1','l2','nest','row','hit'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "row.classList.add('sel')",
            "row.classList.add('zz')",
            "row.classList.remove('sel')",
            "root.classList.add('dark')",
            "a.classList.add('a')",
            "hit.classList.add('hit')",
            "l2.classList.add('pick')",
            "l1.classList.add('pick')",
            "wrap.classList.add('off')",
            "nest.classList.add('nest')",
            "row.className='other'",
            "root.classList.remove('dark')",
            "wrap.classList.remove('off')",
            "l1.classList.remove('pick')",
            "hit.classList.remove('hit')",
            "leaf.classList.add('on')",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        #[cfg(debug_assertions)]
        assert!(host.borrow().pages[&1].state.styles.cache.incremental >= 15);
    }

    #[test]
    fn incremental_styles_follow_added_rules_and_places() {
        let (host, engine) = engine("<html><head><style>li:last-child{padding-left:1px} li:nth-child(odd){padding-right:2px}
                    li+li{padding-top:3px} ul:has(.hit) b{color:red}</style><style>@import 'tailwindcss';</style></head>
                    <body><ul id=list><li id=one class=late><b>1</b></li><li><b>2</b></li><li><b>3</b></li></ul>
                    <p id=leaf class='flex late'>x</p></body></html>");
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['list','one','leaf'])globalThis[id]=document.getElementById(id);globalThis.sheet=document.styleSheets[0];globalThis.css=(e,p)=>getComputedStyle(e)[p];{read}"), "test")
            .unwrap();
        let mut passes = Vec::new();
        for (step, check) in [
            (
                "sheet.insertRule('.late{color:rgb(1, 1, 1)}', sheet.cssRules.length)",
                "css(leaf,'color')==='rgb(1, 1, 1)'&&css(one.firstChild,'color')==='rgb(1, 1, 1)'",
            ),
            (
                "sheet.insertRule('#one b{color:rgb(2, 2, 2)}', 0)",
                "css(one.firstChild,'color')==='rgb(2, 2, 2)'",
            ),
            (
                "const s=document.createElement('style');s.textContent='li:first-child{margin-top:4px} .late::before{content:\"x\";color:rgb(3, 3, 3)}';document.head.append(s)",
                "css(one,'marginTop')==='4px'&&getComputedStyle(leaf,'::before').color==='rgb(3, 3, 3)'",
            ),
            (
                "leaf.classList.add('pt-[3px]')",
                "css(leaf,'paddingTop')==='3px'",
            ),
            (
                "list.append(document.createElement('li'))",
                "css(list.lastChild,'paddingLeft')==='1px'&&css(list.children[2],'paddingLeft')==='0px'&&css(list.lastChild,'paddingRight')==='0px'",
            ),
            (
                "list.prepend(document.createElement('li'))",
                "css(one,'marginTop')==='0px'&&css(one,'paddingRight')==='0px'&&css(one,'paddingTop')==='3px'&&css(list.firstChild,'paddingTop')==='0px'",
            ),
            (
                "list.lastChild.className='hit'",
                "css(one.firstChild,'color')==='rgb(2, 2, 2)'&&css(list.children[2].firstChild,'color')==='rgb(255, 0, 0)'",
            ),
            (
                "list.lastChild.remove()",
                "css(list.children[2].firstChild,'color')==='rgb(1, 1, 1)'||css(list.children[2].firstChild,'color')==='rgb(0, 0, 0)'",
            ),
            (
                "sheet.insertRule('*{margin-left:5px}', 0)",
                "css(leaf,'marginLeft')==='5px'",
            ),
            ("sheet.deleteRule(0)", "css(leaf,'marginLeft')==='0px'"),
        ] {
            let before = host.borrow().pages[&1].state.styles.cache.incremental;
            let result = engine
                .eval(&format!("{step};{read}{check}"), "test")
                .unwrap();
            assert_eq!(result, "true", "{step}");
            passes.push(host.borrow().pages[&1].state.styles.cache.incremental > before);
        }
        // A rule that any node can match needs a full pass.
        assert_eq!(
            passes,
            [true, true, true, true, true, true, true, true, false, true]
        );
    }

    #[test]
    fn incremental_styles_renumber_counters() {
        let (host, engine) = engine("<html><head><style>main{counter-reset:sec} h2{counter-increment:sec}
                    h2::before{content:counter(sec) \". \"} h2::after{content:\"counter \" counters(sec, \"-\")}
                    .big{font-size:20px}</style></head><body><main id=main><h2 id=a>a</h2><p id=p>p</p><h2 id=b>b</h2>
                    <section id=s><h2 id=c>c</h2></section></main></body></html>");
        // `getComputedStyle()` gives the declared counters, so the text comes from the styles.
        let read =
            "[...document.querySelectorAll('h2')].map(h=>getComputedStyle(h).color&&h._id).join()";
        let text = |ids: String| {
            let host = host.borrow();
            let computed = host.pages[&1].state.styles.computed.as_ref().unwrap();
            ids.split(',')
                .map(|id| {
                    let pseudos = &computed.pseudos[&id.parse::<usize>().unwrap()];
                    pseudos
                        .iter()
                        .map(|style| style.get(Prop::CONTENT).unwrap_or_default())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("|")
        };
        let numbers = |list: &[&str]| -> String {
            let text = |n: &&str| {
                let last = n.rsplit('-').next().unwrap();
                format!("\"{last}\" \". \"\"counter \" \"{n}\"")
            };
            list.iter().map(text).collect::<Vec<_>>().join("|")
        };
        engine
            .eval(&format!("for(const id of ['main','a','p','b','s','c'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        let mut passes = Vec::new();
        for (step, expected) in [
            ("p.className='big'", &["1", "2", "3"][..]),
            (
                "main.prepend(document.createElement('h2'))",
                &["1", "2", "3", "4"],
            ),
            ("b.remove()", &["1", "2", "3"]),
            ("s.style.counterReset='sec 7'", &["1", "2", "2-8"]),
            ("s.style.counterReset=''", &["1", "2", "3"]),
            ("a.className='big'", &["1", "2", "3"]),
        ] {
            let before = host.borrow().pages[&1].state.styles.cache.incremental;
            let result = text(engine.eval(&format!("{step};{read}"), "test").unwrap());
            assert_eq!(result, numbers(expected), "{step}");
            passes.push(host.borrow().pages[&1].state.styles.cache.incremental > before);
        }
        assert_eq!(passes, [true; 6]);
    }

    #[test]
    fn cousins_that_share_matches_restyle_with_their_own_ancestors() {
        let rows = "<div class=row><span class=x>1</span></div><div class=row><span class=x>2</span></div>".repeat(2);
        let (host, engine) = engine(&format!(
                    "<html><head><style>.a .x{{color:red}} .g[data-k=v] .x{{padding-left:1px}} .row.on .x{{color:blue}}
                    .g:hover .x{{color:green}}</style></head><body><div id=list><div id=g1 class=g>{rows}</div>
                    <div id=g2 class=g>{rows}</div><div id=g3 class=g>{rows}</div></div></body></html>"
                ));
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['g1','g2','g3'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "g2.classList.add('a')",
            "g3.children[3].classList.add('on')",
            "g1.dataset.k='v'",
            "g2.children[1].classList.add('on')",
            "g2.classList.remove('a')",
            "g3.append(g1.children[0])",
            "g1.dataset.k='w'",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        let g3 = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "g3"))
            .unwrap();
        host.borrow_mut()
            .call(Role::Renderer(1), "dom.hover", json!([[g3]]))
            .unwrap();
        engine.eval(read, "test").unwrap();
        #[cfg(debug_assertions)]
        assert!(host.borrow().pages[&1].state.styles.cache.incremental >= 8);
    }

    #[test]
    fn incremental_styles_adjust_collapsed_tables_again() {
        let rows = "<tr><td>a</td><td>b</td></tr>".repeat(4);
        let (host, engine) = engine(&format!(
                    "<html><head><style>table{{border-collapse:collapse}} td{{border:1px solid #ccc}}
                    tr.danger td{{background-color:#fdd}} tr.thick td{{border-bottom:3px solid red}} tr.wide{{border-top:4px solid blue}}
                    .x{{border-left:5px double green}} .blocky{{display:block}}</style></head><body><table id=t><thead><tr><th>h</th><th>i</th></tr></thead>
                    <tbody id=b>{rows}</tbody></table><table id=u style='border:2px solid'><tr><td>c</td></tr></table></body></html>"
                ));
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!("for(const id of ['t','b','u'])globalThis[id]=document.getElementById(id);{read}"), "test")
            .unwrap();
        for step in [
            "b.rows[1].className='danger'",
            "b.rows[1].className='thick'",
            "b.rows[2].className='wide'",
            "b.rows[0].cells[1].className='x'",
            "b.rows[3].cells[0].colSpan=2",
            "b.rows[3].deleteCell(1)",
            "b.insertRow().insertCell().textContent='n'",
            "b.rows[0].remove()",
            "t.style.border='6px solid'",
            "u.rows[0].insertCell().style.border='7px dashed'",
            "b.rows[1].className=''",
            "t.style.borderCollapse='separate'",
            "t.style.borderCollapse=''",
            "t.classList.add('blocky')",
            "t.classList.remove('blocky')",
            "b.rows[0].className='danger'",
        ] {
            engine.eval(&format!("{step};{read}"), "test").unwrap();
        }
        #[cfg(debug_assertions)]
        assert!(host.borrow().pages[&1].state.styles.cache.incremental >= 12);
    }

    #[test]
    fn incremental_styles_match_full_passes_in_shadow_trees() {
        let (host, engine) = engine("<html><head><style>
                    :root{--ink:navy} x-card.big{font-size:20px} x-card::part(title){color:green}
                    x-card.alt::part(title){color:olive} x-card::part(deep){padding-left:3px}
                    #b{margin-top:1px} .late{color:maroon}
                    </style></head><body><x-card id=card class=c><span id=a class=on>a<s id=s></s></span>
                    <b id=b slot=side>b</b><i id=i>i</i></x-card><x-card id=other></x-card>
                    <x-card id=third></x-card><div id=late class=late><p id=lp>late</p></div></body></html>");
        let card = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").is_some_and(|id| id == "card"))
            .unwrap();
        let read = "for(const e of document.querySelectorAll('*'))getComputedStyle(e).color;";
        engine
            .eval(&format!(r#"
                for (const id of ['card','a','b','i','other','third','late']) globalThis[id] = document.getElementById(id);
                globalThis.root = card.attachShadow({{mode:'open'}});
                root.innerHTML = '<style>:host{{display:block;color:var(--ink)}} :host(.on){{border-top:1px solid red}} :host(:hover){{color:orange}} ::slotted(.on){{color:red}} slot[name=side]::slotted(*){{padding-top:2px}} .t{{font-weight:700;font-size:2em}} slot{{color:var(--tone,inherit)}} slot.wide::slotted(*){{padding-left:9px}} :host(:has(> .flag)){{padding-right:2px}} ::slotted(:first-child){{padding-bottom:4px}} ::slotted(:has(> .mark)){{padding-right:5px}}</style><h1 part=title class=t id=t>t</h1><slot id=main></slot><slot name=side id=side>fallback</slot><x-inner id=inner exportparts="core: deep"></x-inner>';
                globalThis.inner = root.getElementById('inner').attachShadow({{mode:'open'}});
                inner.innerHTML = '<style>:host{{display:block}} b{{color:var(--ink)}}</style><b part=core id=core>c</b>';
                globalThis.sheet = new CSSStyleSheet();
                sheet.replaceSync(':host{{margin-left:4px}} p{{color:teal}}');
                for (const host of [other, third]) {{ const shadow = host.attachShadow({{mode:'open'}}); shadow.adoptedStyleSheets = [sheet]; shadow.innerHTML = '<p>o</p>'; }}
                {read}"#), "test")
            .unwrap();
        // Only debug builds count the incremental passes.
        #[cfg(debug_assertions)]
        let mut full = Vec::new();
        for step in [
            "card.classList.add('on')",
            "card.classList.add('alt')",
            "card.classList.add('big')",
            "document.documentElement.style.setProperty('--ink','purple')",
            "card.style.setProperty('--ink','teal')",
            "card.style.setProperty('--tone','gray')",
            "a.classList.remove('on')",
            "a.classList.add('on')",
            "root.getElementById('main').classList.add('wide')",
            "root.getElementById('main').classList.remove('wide')",
            "a.classList.add('flag')",
            "a.classList.remove('flag')",
            "card.prepend(document.createElement('u'))",
            "document.getElementById('s').classList.add('mark')",
            "document.getElementById('s').classList.remove('mark')",
            "b.slot=''",
            "b.slot='side'",
            "root.getElementById('side').setAttribute('name','none')",
            "root.getElementById('side').setAttribute('name','side')",
            "root.getElementById('t').setAttribute('part','other')",
            "root.getElementById('t').setAttribute('part','title')",
            "root.getElementById('inner').setAttribute('exportparts','core')",
            "root.getElementById('inner').setAttribute('exportparts','core: deep')",
            "inner.getElementById('core').classList.add('x')",
            "card.append(document.createElement('em'))",
            "root.append(document.createElement('div'))",
            "i.remove()",
            "card.prepend(i)",
            "other.remove()",
            "document.body.append(other)",
            "late.attachShadow({mode:'open'}).adoptedStyleSheets=[sheet]",
            "late.shadowRoot.innerHTML='<slot></slot><p>s</p>'",
            "third.remove()",
            "card.classList.remove('big')",
            "card.classList.remove('on')",
            "sheet.replaceSync(':host{margin-left:5px} p{color:red}')",
            "card.classList.add('on')",
        ] {
            #[cfg(debug_assertions)]
            let before = host.borrow().pages[&1].state.styles.cache.incremental;
            engine.eval(&format!("{step};{read}"), "test").unwrap();
            #[cfg(debug_assertions)]
            if host.borrow().pages[&1].state.styles.cache.incremental == before {
                full.push(step);
            }
        }
        for ids in [vec![card], vec![]] {
            host.borrow_mut()
                .call(Role::Renderer(1), "dom.hover", json!([ids]))
                .unwrap();
            engine.eval(read, "test").unwrap();
        }
        #[cfg(debug_assertions)]
        assert_eq!(
            full,
            ["sheet.replaceSync(':host{margin-left:5px} p{color:red}')"]
        );
        let colors = engine
            .eval(
                "JSON.stringify([getComputedStyle(card).color, getComputedStyle(a).color, getComputedStyle(root.getElementById('t')).color, getComputedStyle(inner.getElementById('core')).paddingLeft, getComputedStyle(late.shadowRoot.querySelector('p')).color])",
                "test",
            )
            .unwrap();
        assert_eq!(
            colors,
            r#"["rgb(0, 128, 128)","rgb(255, 0, 0)","rgb(128, 128, 0)","3px","rgb(255, 0, 0)"]"#
        );
    }

    #[test]
    fn runtime_stylesheets_reuse_parses_and_evict_old_text() {
        let mut doc = Document::parse(
            &(0..200)
                .map(|index| format!("<style>.item{index}{{color:red}}</style>"))
                .collect::<String>(),
        );
        let mut state = PageState::default();
        state.restyle(&mut doc, [800., 600.]);
        let sheets = state.styles.sheets.clone();
        assert_eq!(sheets.len(), 200);
        state.styles.computed = None;
        state.restyle(&mut doc, [800., 600.]);
        for (source, sheet) in sheets {
            assert!(Rc::ptr_eq(&sheet, &state.styles.sheets[&source]));
        }
        let sheet = doc
            .nodes
            .iter()
            .position(|node| node.tag == "style")
            .unwrap();
        let text = doc.nodes[sheet].children[0];
        doc.nodes[text].text = ".replacement{color:blue}".into();
        state.styles.computed = None;
        state.restyle(&mut doc, [800., 600.]);
        assert_eq!(state.styles.sheets.len(), 200);
        assert!(!state.styles.sheets.contains_key(".item0{color:red}"));
        let head = doc.nodes[sheet].parent.unwrap();
        doc.nodes[head].children.retain(|id| *id != sheet);
        doc.nodes[sheet].parent = None;
        state.styles.computed = None;
        state.restyle(&mut doc, [800., 600.]);
        assert_eq!(state.styles.sheets.len(), 199);
        assert!(!state.styles.sheets.contains_key(".replacement{color:blue}"));
    }

    #[test]
    fn registrations_restyle_the_nodes_that_read_them() {
        let mut doc = Document::parse(
            "<style>p{width:var(--w,5px)}b{--x:1}b span{height:var(--x,2px)}i{color:red}</style><p>x</p><b><span>y</span></b><i>z</i>",
        );
        let mut state = PageState::default();
        state.restyle(&mut doc, [800., 600.]);
        for (name, initial) in [("--w", "initial-value:7px"), ("--x", "")] {
            let rule = format!("@property {name}{{syntax:\"*\";inherits:false;{initial}}}");
            let value = css::sheet::Stylesheet::parse(&rule).property().unwrap();
            state
                .styles
                .cache
                .scripted
                .insert(css::properties::Prop::new(name), value);
        }
        state.styles.invalidate();
        let computed = state.restyle(&mut doc, [800., 600.]);
        let find = |tag: &str| doc.nodes.iter().position(|node| node.tag == tag).unwrap();
        assert_eq!(&computed.values[find("p")]["width"], "7px");
        assert_eq!(&computed.values[find("span")]["height"], "2px");
        assert_eq!(&computed.values[find("i")]["color"], "red");
    }

    #[test]
    fn replaced_style_text_keeps_the_other_sheets() {
        let mut doc = Document::parse(
            "<style>p{color:red}</style><style>.a{color:green}.a+p{width:1px}</style><style>b{color:black}</style><p class=a>x</p><p>y</p><b>z</b><i>w</i>",
        );
        let find = |doc: &Document, tag: &str, nth: usize| {
            let mut found = doc
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.tag == tag);
            found.nth(nth).unwrap().0
        };
        let mut state = PageState::default();
        state.restyle(&mut doc, [800., 600.]);
        let sheet = find(&doc, "style", 1);
        let text = doc.nodes[sheet].children[0];
        doc.nodes[text].text = ".a{color:blue}".into();
        doc.nodes[sheet].stylesheet = None;
        state.styles.invalidate();
        let computed = state.restyle(&mut doc, [800., 600.]);
        let [a, p, b] = [find(&doc, "p", 0), find(&doc, "p", 1), find(&doc, "b", 0)];
        assert_eq!(&computed.values[a]["color"], "blue");
        assert_eq!(&computed.values[p]["color"], "red");
        assert_eq!(computed.values[p].get(Prop::WIDTH), None);
        assert_eq!(&computed.values[b]["color"], "black");
    }

    #[test]
    fn removed_nodes_drop_caches_and_style_again_when_they_return() {
        let mut doc = Document::parse(
            "<style>.on p{color:red}p:first-child{width:1px}</style><div class=on><p>x</p></div><div id=b></div>",
        );
        let find = |doc: &Document, tag: &str| doc.nodes.iter().position(|node| node.tag == tag);
        let p = find(&doc, "p").unwrap();
        let on = doc.nodes[p].parent.unwrap();
        let mut state = PageState::default();
        let viewport = [800., 600.];
        state.restyle(&mut doc, viewport);
        doc.nodes[on].children.retain(|&id| id != p);
        doc.nodes[p].parent = None;
        state.tree.removed.push(p);
        state.styles.invalidate();
        state.restyle(&mut doc, viewport);
        assert!(state.tree.removed.is_empty());
        doc.nodes[on].children.push(p);
        doc.nodes[p].parent = Some(on);
        state.styles.invalidate();
        let returned = state.restyle(&mut doc, viewport);
        let fresh = PageState::default().restyle(&mut doc, viewport);
        assert_eq!(returned.values[p], fresh.values[p]);
        assert_eq!(returned.values[p].get(Prop::COLOR), Some("red"));
    }

    #[test]
    fn new_nodes_grow_cached_styles_like_a_full_pass() {
        let mut doc = Document::parse("<p style='font-size:30px;--x:1'>a</p>");
        let mut state = PageState::default();
        let viewport = [800., 600.];
        state.restyle(&mut doc, viewport);
        let id = state.allocate(
            &mut doc,
            Node {
                tag: "div".into(),
                ..Default::default()
            },
        );
        let read = |doc: &mut Document, state: &mut PageState| {
            call(
                "dom.computed",
                &json!([id, "color", ""]),
                doc,
                state,
                viewport,
            )
            .unwrap()
        };
        assert_eq!(read(&mut doc, &mut state), json!(""));
        let cached = state.restyle(&mut doc, viewport);
        let fresh = PageState::default().restyle(&mut doc, viewport);
        assert_eq!(cached.vars.len(), doc.nodes.len());
        for node in 0..doc.nodes.len() {
            assert!(cached.lengths[node] == fresh.lengths[node], "{node}");
        }
        let p = (0..doc.nodes.len())
            .find(|&node| doc.nodes[node].tag == "p")
            .unwrap();
        doc.nodes[p].attrs.insert("class".into(), "x".into());
        state.styles.invalidate();
        state.restyle(&mut doc, viewport);
    }

    #[test]
    fn trim_cuts_the_free_end_of_the_arena_and_ids_come_back_lowest_first() {
        let mut doc = Document::parse("<div></div>");
        let mut state = PageState::default();
        let viewport = [800., 600.];
        let node = || Node {
            tag: "p".into(),
            ..Default::default()
        };
        let ids: Vec<usize> = (0..4).map(|_| state.allocate(&mut doc, node())).collect();
        state.restyle(&mut doc, viewport);
        for id in [ids[3], ids[1], ids[2]] {
            doc.nodes[id] = Node::default();
            state.tree.free.push(std::cmp::Reverse(id));
        }
        assert!(state.trim(&mut doc));
        assert_eq!(doc.nodes.len(), ids[1]);
        assert_eq!(state.tree.free.len(), 0);
        assert!(!state.trim(&mut doc));
        state.restyle(&mut doc, viewport);
        assert_eq!(state.allocate(&mut doc, node()), ids[1]);
        assert_eq!(state.allocate(&mut doc, node()), ids[1] + 1);
        doc.nodes[ids[1]] = Node::default();
        state.tree.free.push(std::cmp::Reverse(ids[1]));
        doc.nodes[ids[0]] = Node::default();
        state.tree.free.push(std::cmp::Reverse(ids[0]));
        assert_eq!(state.allocate(&mut doc, node()), ids[0]);
    }

    #[test]
    fn reused_ids_drop_the_old_style_while_the_styles_are_shared() {
        let mut doc =
            Document::parse("<body><div><p style='color: rgb(255, 0, 0)'></p></div></body>");
        let mut state = PageState::default();
        let viewport = [800., 600.];
        let p = doc.nodes.iter().position(|node| node.tag == "p").unwrap();
        let div = doc.nodes[p].parent.unwrap();
        state.restyle(&mut doc, viewport);
        let body = doc.nodes[div].parent.unwrap();
        doc.nodes[body].children.clear();
        doc.nodes[div].parent = None;
        state.styles.invalidate();
        let shared = state.restyle(&mut doc, viewport);
        assert_eq!(shared.values[p].get(Prop::COLOR), None);
        for id in [p, div] {
            doc.nodes[id] = Node::default();
            state.tree.free.push(std::cmp::Reverse(id));
        }
        let node = Node {
            tag: "p".into(),
            ..Default::default()
        };
        assert_eq!(state.allocate(&mut doc, node.clone()), div);
        assert_eq!(state.allocate(&mut doc, node), p);
        assert_eq!(
            state.restyle(&mut doc, viewport).values[p].get(Prop::COLOR),
            None
        );
    }

    #[test]
    fn trim_shrinks_the_layout_mirror_under_a_page_that_holds_it() {
        let (host, engine) = engine("<html><body><p id='a'></p></body></html>");
        let a = host.borrow().pages[&1]
            .doc
            .nodes
            .iter()
            .position(|node| node.tag == "p")
            .unwrap();
        let grow = |rect| {
            let mut host = host.borrow_mut();
            let host = &mut *host;
            let page = host.pages.get_mut(&1).unwrap();
            let (doc, state) = (&mut page.doc, &mut page.state);
            let ids: Vec<usize> = (0..20_000)
                .map(|_| state.allocate(doc, Node::default()))
                .collect();
            state.geometry.mirror.insert(a, rect);
            state.geometry.mirror.insert(ids[ids.len() - 1], rect);
            ids
        };
        let ids = grow([0., 0., 10., 10.]);
        assert_eq!(
            engine
                .eval(
                    "globalThis.a = document.getElementById('a'); a.offsetWidth",
                    "test"
                )
                .unwrap(),
            "10"
        );
        {
            let mut host = host.borrow_mut();
            let host = &mut *host;
            let page = host.pages.get_mut(&1).unwrap();
            let (doc, state) = (&mut page.doc, &mut page.state);
            for &id in &ids {
                state.tree.free.push(std::cmp::Reverse(id));
            }
            assert!(state.trim(doc));
            assert_eq!(state.geometry.mirror.capacity, 1024);
            assert_eq!(state.geometry.mirror.get(&a), Some(&[0., 0., 10., 10.]));
            state.geometry.mirror.insert(a, [0., 0., 30., 10.]);
            state.geometry.mirror.touch();
        }
        assert_eq!(engine.eval("a.offsetWidth", "test").unwrap(), "30");
        grow([0., 0., 40., 10.]);
        assert_eq!(engine.eval("a.offsetWidth", "test").unwrap(), "40");
    }
}
