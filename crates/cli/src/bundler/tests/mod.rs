use super::*;
use super::{
    css::css,
    html::urls,
    resolve::{BUILTINS, PACKAGES},
};

fn linked_css(page: &str) -> String {
    Document::parse(page)
        .nodes
        .iter()
        .find_map(|node| node.attrs.get(apps_runtime::bundle::LINK_SHEET).cloned())
        .unwrap_or_default()
}

mod cache;
mod html_css;
mod modules;
mod resolve;
mod runtime;
mod workers;
