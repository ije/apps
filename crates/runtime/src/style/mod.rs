use crate::css::SelectorList;
use crate::css::{
    Declaration, Declarations, Lengths, Prop, Rule, Style, Stylesheet, color, components,
};
use crate::dom::Document;
use bbtree::unsync::BBTreeMap;
use std::collections::HashMap;

mod animation;
mod cascade;
mod chunked;
mod fonts;
mod keyframes;
mod page;
pub(crate) mod tailwind;
#[cfg(test)]
mod tests;
mod text;
mod ua;
mod used;
mod vars;
pub(crate) use cascade::computed::*;
pub(crate) use cascade::{Shadows, StyleCache, StyleContext, compute_styles};
pub(crate) use chunked::Chunked;
pub(crate) use fonts::{FONT_NAMES, font_lists, font_names};
pub(crate) use page::{Page, ShadowWalk, Styles};
pub(crate) use ua::{SVG, presentation};

pub(crate) use animation::{Frame, Motion, Scripted};
pub(crate) use text::{
    LineHeight, case, case_ignorable, cursor, font, line_height, pseudo_content, push_full,
    restyle, selection_colors, spacing, starts_word, styled_run, text_run, text_transform,
    word_break,
};
pub(crate) use used::{dark, drawable_canvas};

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
