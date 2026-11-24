use crate::css::SelectorList;
use crate::css::{
    Declaration, Declarations, FontUnits, Lengths, Prop, Rule, Style, Stylesheet, Vars, color,
    components,
};
use crate::dom::Document;
use bbtree::unsync::BBTreeMap;
use std::collections::HashMap;

mod animation;
mod cascade;
mod chunked;
mod computed;
mod fonts;
mod page;
pub(crate) mod tailwind;
#[cfg(test)]
mod tests;
mod text;
mod ua;
pub(crate) use cascade::{
    Shadows, StyleCache, StyleContext, compute_styles, dark, drawable_canvas,
};
pub(crate) use chunked::Chunked;
pub(crate) use computed::*;
pub(crate) use fonts::{FONT_NAMES, font_lists, font_names};
pub(crate) use page::{Page, ShadowWalk, Styles};
pub(crate) use ua::{SVG, presentation};

pub(crate) use animation::{Motion, Scripted};
pub(crate) use text::{
    Atom, CUT_AFTER, CUT_BEFORE, CUT_FIRST, CUT_LAST, InlineBox, InlineText, LineHeight, atom_key,
    cursor, font, holds_block_box, inline_text, line_height, makes_no_line, own_box,
    placed_out_of_flow, pseudo_blocks, pseudo_content, restyle, selection_colors, spacing, spliced,
    static_after, static_inline, text_run, text_transform, word_break,
};

pub fn styles(doc: &Document, width: f32, height: f32) -> Vec<BBTreeMap<String, String>> {
    let out = compute_styles(
        doc,
        width,
        height,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    );
    out.values
        .iter()
        .zip(out.vars.iter())
        .map(|(style, vars)| {
            style
                .own()
                .chain(vars.iter().map(|(prop, value)| (*prop, value)))
                .map(|(prop, value)| (prop.name().to_owned(), value.to_string()))
                .collect()
        })
        .collect()
}
