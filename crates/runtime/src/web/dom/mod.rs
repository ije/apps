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
    let id = || -> Result<usize> {
        let id = args[0].as_u64().context("Expected node id")? as usize;
        ensure!(id < doc.nodes.len(), "Invalid node id");
        Ok(id)
    };
    match op {
        "dom.cssControl" => css::css_control(args, doc, state, viewport),
        "dom.easing" => css::easing(args),
        "dom.animation" => css::animation(args, doc, state),
        "dom.mediaMatches" => css::media_matches(args, state, viewport),
        "dom.properties" => css::properties(),
        "dom.aliases" => css::aliases(),
        "dom.keyword" => css::keyword(args),
        "dom.path" => css::css_path(args),
        "dom.specified" => css::specified(args),
        "dom.popoverSource" => {
            let id = id()?;
            state.layout.anchors.opened += 1;
            let sources = &mut state.layout.anchors.sources;
            match args[1].as_u64().filter(|&source| source != 0) {
                Some(source) => sources.insert(id, source as usize),
                None => sources.remove(&id),
            };
            Ok(Value::Null)
        }
        "dom.declare" => css::declare(args),
        "dom.constructed" | "dom.adopt" | "dom.supports" | "dom.registerProperty" => {
            css::constructed(op, args, state)
        }
        "dom.cssAnimations" => css::css_animations(id()?, args, doc, state, viewport),
        "dom.animated" => css::animated(id()?, args, doc, state, viewport),
        "dom.computed" => css::computed(id()?, args, doc, state, viewport),
        "dom.css" => css::css(id()?, doc, state),
        "dom.sheet" => css::sheet(id()?, args, doc, state),
        "dom.font" => {
            id()?;
            css::font(args, state)
        }
        "dom.fontSources" => {
            id()?;
            css::font_sources(args)
        }
        "dom.fontCheck" => {
            id()?;
            css::font_check(args, state)
        }
        "dom.scrollEvents" => geometry::scroll_events(state),
        "dom.hitTest" => geometry::hit_test(args, doc, state, viewport),
        "dom.scrolled" => geometry::scrolled(id()?, state),
        "dom.snapping" => Ok(json!(
            (state.geometry.scroll.get(&id()?)).is_some_and(renderer::ScrollRef::snapping)
        )),
        "dom.scroll" => geometry::scroll(id()?, args, doc, state, viewport),
        "dom.fragments" => geometry::fragments(id()?, state),
        "dom.client" => geometry::client(id()?, args, doc, state, viewport),
        "dom.skipped" => geometry::skipped(id()?, args, doc, state, viewport),
        "dom.decoded" => media::decoded(state),
        "dom.frames" => media::frames(state),
        "dom.canvas" => media::canvas(id()?, args, doc, state),
        "dom.canvasURL" => media::canvas_url(id()?, args, doc, state),
        "dom.canvasPixels" => media::canvas_pixels(id()?, args, doc, state),
        "dom.canvasTake" => media::canvas_take(id()?, args, doc, state),
        "dom.canvasAdopt" => media::canvas_adopt(id()?, args, doc, state),
        "dom.snapshot" => media::snapshot(id()?, args, state),
        "dom.drawn" => media::drawn(id()?, args, state),
        "dom.canvasLink" => media::canvas_link(id()?, state),
        "dom.canvasBind" => media::canvas_bind(id()?, args, state),
        "dom.image" => media::image(id()?, args, state),
        "dom.canvasFont" => {
            id()?;
            media::canvas_font(args)
        }
        "dom.canvasColor" => {
            id()?;
            media::canvas_color(args)
        }
        "dom.svgBox" => media::svg_box(id()?, doc, state),
        "dom.measureText" => {
            id()?;
            media::measure_text(args, state)
        }
        "dom.canvasQuery" => media::canvas_query(id()?, args, state),
        "dom.release" => nodes::release(args, doc, state),
        "dom.focus" | "dom.hover" | "dom.active" => nodes::focus(op, args, doc, state),
        "dom.target" => nodes::target(args, doc, state),
        "dom.reserve" => nodes::reserve(args, doc, state),
        "dom.assignedNodes" => nodes::assigned_nodes(id()?, args, doc, state),
        "dom.tree" => nodes::subtree(id()?, doc),
        "dom.source" => nodes::source(id()?, doc),
        "dom.matches" => nodes::matches(id()?, args, doc, state),
        "dom.closest" => nodes::closest(id()?, args, doc, state),
        "dom.queryWithin" => nodes::query_within(id()?, args, doc, state),
        "dom.html" => nodes::html(id()?, args, doc, state),
        "dom.rangeNodes" => text::range_nodes(args, doc),
        "dom.selection" => text::selection(id()?, args, doc, state),
        "dom.highlights" => text::highlights(args, doc, state),
        "dom.userSelect" | "dom.selectionText" => {
            text::user_select(op, id()?, args, doc, state, viewport)
        }
        "dom.textOffset" | "dom.textPoint" => text::text_offset(op, id()?, args, doc),
        "dom.textRect" | "dom.textIndex" => text::text_rect(op, id()?, args, doc, state),
        "dom.innerText" => text::inner_text(id()?, doc, state, viewport),
        "dom.caretText" => text::caret(id()?, args, doc, state, viewport),
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
