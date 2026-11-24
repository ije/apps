use crate::css::{Declared, SelectorList};
use crate::style::cascade::computed::Shadowed;
use crate::style::cascade::container::ContainerQuery;
use crate::style::cascade::rank::Layer;
use std::rc::Rc;

pub(crate) fn exported_parts(
    hosted: Vec<Passed>,
    own: Option<&Shadowed>,
    outer: Option<&Shadowed>,
    names: Option<&String>,
    old: Option<&Shadowed>,
) -> Option<Shadowed> {
    let mut list = hosted;
    list.extend(own.into_iter().flat_map(|own| own.iter().cloned()));
    if let (Some(outer), Some(names)) = (outer, names) {
        let pairs: Vec<(&str, &str)> = names
            .split(',')
            .filter_map(|item| {
                let (inner, name) = item.split_once(':').unwrap_or((item, item));
                let (inner, name) = (inner.trim(), name.trim());
                (!inner.is_empty() && !name.is_empty()).then_some((inner, name))
            })
            .collect();
        for passed in outer.iter().filter(|passed| passed.rule.context > 0) {
            let mut inner = vec![String::new()];
            for name in passed.name.split_ascii_whitespace() {
                inner = inner
                    .iter()
                    .flat_map(|done| {
                        pairs
                            .iter()
                            .filter(move |pair| pair.1 == name)
                            .map(move |pair| format!("{done} {}", pair.0))
                    })
                    .take(64)
                    .collect();
            }
            list.extend(
                inner
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .map(|name| {
                        let mut passed = passed.clone();
                        passed.name = name.trim_start().into();
                        passed.rule.context = passed.rule.context.saturating_add(1);
                        passed
                    }),
            );
        }
    }
    (!list.is_empty()).then(|| match old {
        Some(old) if **old == list => old.clone(),
        _ => Rc::new(list),
    })
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(crate) struct ShadowRule {
    pub(crate) kind: usize,
    pub(crate) context: i8,
    pub(crate) layer: Layer,
    pub(crate) specificity: [usize; 3],
    pub(crate) order: u128,
    pub(crate) declarations: Rc<[Declared]>,
    pub(crate) query: Option<(usize, usize, bool)>,
}

pub(crate) const ELEMENT: usize = usize::MAX;
// Pseudo-element kinds of `Selector::without_pseudo_element`.
pub(crate) const SLOTTED: usize = 12;
pub(crate) const PART: usize = 13;
pub(crate) const STARTING: usize = 14;

#[derive(Clone)]
pub(crate) struct Passed {
    pub(crate) name: Rc<str>,
    pub(crate) compound: SelectorList,
    pub(crate) conditions: Option<(usize, usize, ContainerQuery)>,
    pub(crate) rule: ShadowRule,
}

impl PartialEq for Passed {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && Rc::ptr_eq(&self.compound, &other.compound)
            && self.rule == other.rule
            && match (&self.conditions, &other.conditions) {
                (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1 && Rc::ptr_eq(&a.2, &b.2),
                (a, b) => a.is_none() && b.is_none(),
            }
    }
}
