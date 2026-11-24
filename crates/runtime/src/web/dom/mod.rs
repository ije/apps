mod css;
pub(crate) mod geometry;
mod media;
mod nodes;
mod text;

use crate::dom::Document;
use crate::render::PageState;
use crate::style;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

// The DOM operations that page scripts call.
pub fn call(
    op: &str,
    args: &Value,
    doc: &mut Document,
    state: &mut PageState,
    viewport: [f32; 2],
) -> Result<Value> {
    if op == "dom.wheel" {
        state.wheel = true;
        return Ok(Value::Null);
    }
    if op == "dom.iterations" {
        state.styles.motion.listen_iterations();
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
    match op {
        "dom.cssControl"
        | "dom.easing"
        | "dom.animation"
        | "dom.mediaMatches"
        | "dom.properties"
        | "dom.aliases"
        | "dom.path"
        | "dom.specified"
        | "dom.declare"
        | "dom.constructed"
        | "dom.adopt"
        | "dom.supports"
        | "dom.registerProperty" => return css::page(op, args, doc, state, viewport),
        "dom.decoded" | "dom.frames" => return media::page(op, state),
        "dom.rangeNodes" => return text::page(op, args, doc),
        "dom.scrollEvents" | "dom.hitTest" => {
            return geometry::page(op, args, doc, state, viewport);
        }
        "dom.release" | "dom.focus" | "dom.hover" | "dom.active" | "dom.target" | "dom.reserve" => {
            return nodes::page(op, args, doc, state);
        }
        _ => {}
    }
    let id = args[0].as_u64().context("Expected node id")? as usize;
    ensure!(id < doc.nodes.len(), "Invalid node id");
    match op {
        "dom.cssAnimations" | "dom.animated" | "dom.computed" | "dom.css" | "dom.sheet"
        | "dom.font" | "dom.fontSources" | "dom.fontCheck" => {
            css::node(op, id, args, doc, state, viewport)
        }
        "dom.canvas" | "dom.canvasURL" | "dom.canvasPixels" | "dom.canvasTake"
        | "dom.canvasAdopt" | "dom.snapshot" | "dom.drawn" | "dom.canvasLink"
        | "dom.canvasBind" | "dom.image" | "dom.canvasFont" | "dom.canvasColor" | "dom.svgBox"
        | "dom.measureText" | "dom.canvasQuery" => media::node(op, id, args, doc, state),
        "dom.selection" | "dom.userSelect" | "dom.selectionText" | "dom.textOffset"
        | "dom.textPoint" | "dom.textRect" | "dom.textIndex" | "dom.innerText"
        | "dom.caretText" => text::node(op, id, args, doc, state, viewport),
        "dom.scrolled" | "dom.scroll" | "dom.fragments" | "dom.client" | "dom.skipped" => {
            geometry::node(op, id, args, doc, state, viewport)
        }
        "dom.assignedNodes" | "dom.tree" | "dom.source" | "dom.matches" | "dom.closest"
        | "dom.queryWithin" | "dom.html" => nodes::node(op, id, args, doc, state),
        _ => anyhow::bail!("Unknown DOM operation: {op}"),
    }
}

fn display<'a>(computed: &style::ComputedStyles, id: usize, value: &'a str) -> &'a str {
    match style::role(computed, id) {
        0 => value,
        role => style::TABLE_DISPLAYS[role as usize],
    }
}

#[cfg(test)]
mod tests;
