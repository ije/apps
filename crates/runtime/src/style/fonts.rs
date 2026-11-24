use crate::css::{sheet::Declaration, sheet::Rule, value::tokens::components};
use crate::dom::Document;
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::rc::Rc;
thread_local! {
    // Retain font-family values used as pointer keys to prevent address reuse.
    pub(crate) static FONT_NAMES: std::cell::RefCell<FontNames> = Default::default();
}

pub(crate) type FontNames = (
    HashMap<String, String>,
    FxHashMap<*const u8, (Option<Rc<str>>, renderer::SharedString)>,
);

pub(crate) fn font_lists(doc: &Document) -> Vec<Vec<String>> {
    let mut lists = Vec::new();
    let mut pending: Vec<&Rule> = doc
        .nodes
        .iter()
        .filter_map(|node| node.stylesheet.as_deref())
        .flat_map(|sheet| sheet.0.iter().map(|rule| &**rule))
        .collect();
    while let Some(rule) = pending.pop() {
        match rule {
            Rule::Style(_, rules)
            | Rule::Media(_, rules)
            | Rule::Layer(_, rules)
            | Rule::Supports(_, _, rules)
            | Rule::Theme(_, rules)
            | Rule::Container(_, _, _, rules)
            | Rule::Starting(rules) => pending.extend(rules),
            Rule::Declarations(values) => {
                for declaration in &values.0 {
                    if let Declaration::Property(name, value) = declaration
                        && (name == "font-family" || name.starts_with("--font"))
                    {
                        lists.push(
                            components(value, ',')
                                .iter()
                                .map(|family| family.trim().trim_matches(['\'', '"']).to_owned())
                                .collect(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    lists
}

pub(crate) fn font_names(system: &renderer::TextSystem, aliases: &HashMap<String, String>) {
    FONT_NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if names.0.is_empty() {
            names.1.clear();
            names.0 = system
                .families()
                .into_iter()
                .map(|name| (name.to_ascii_lowercase(), name))
                .chain(
                    aliases
                        .iter()
                        .map(|(alias, name)| (alias.to_ascii_lowercase(), name.clone())),
                )
                .collect();
        }
    });
}
