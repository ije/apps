use super::*;
use crate::style::tailwind::tables::VARIANTS;
use crate::style::tailwind::value::{
    escape, integer, modifier, named_value, negate, quote, split, statements, universal,
};

impl Config {
    pub(super) fn variant(&self, name: &str, depth: u8) -> Option<(Rank, Variant)> {
        if depth > 8 {
            return None;
        }
        let order = |name: &str| VARIANTS.iter().position(|(other, _)| *other == name);
        let (name, modifier) = modifier(name);
        if let Some(raw) = name.strip_prefix('[') {
            if modifier.is_some() || raw.starts_with('@') && raw.contains('&') {
                return None;
            }
            let mut value = self.arbitrary(name)?;
            if value.contains(['{', '}']) {
                return None;
            }
            let relative = value.starts_with(['>', '+', '~']);
            let source = if relative {
                format!(" {} {}", &value[..1], value[1..].trim_start())
            } else if let Some(at) = value.strip_prefix('@') {
                let end = at
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .unwrap_or(at.len());
                format!("@{} {}", &at[..end], at[end..].trim_start())
            } else if value.contains('&') {
                value.clone()
            } else {
                value = format!("&:is({value})");
                value.clone()
            };
            let mut variant = template(&source);
            variant.relative = relative;
            return Some(((u16::MAX, value), variant));
        }
        if let Some(index) = self.variants.iter().position(|(other, _)| other == name) {
            let rank = order(name).unwrap_or(VARIANTS.len() + index) as u16;
            return modifier
                .is_none()
                .then(|| ((rank, String::new()), template(&self.variants[index].1)));
        }
        if let Some(index) = order(name)
            && !VARIANTS[index].1.is_empty()
        {
            if modifier.is_some() {
                return None;
            }
            let mut variant = template(VARIANTS[index].1);
            if name == "hover" {
                variant.branches[0].0.push("@media (hover: hover)".into());
            }
            variant.content = matches!(name, "before" | "after");
            variant.never |= matches!(name, "*" | "**");
            return Some(((index as u16, String::new()), variant));
        }
        if modifier.is_none()
            && let Some(width) = self.raw(&format!("--breakpoint-{name}"))
        {
            return self.media(">=", &width, "min");
        }
        let (root, value) = if let Some(value) = name.strip_prefix("@min-") {
            ("@min", value)
        } else if let Some(value) = name.strip_prefix("@max-") {
            ("@max", value)
        } else if let Some(value) = name.strip_prefix('@') {
            ("@min", value)
        } else {
            VARIANTS
                .iter()
                .filter(|(_, template)| template.is_empty())
                .filter_map(|(root, _)| Some((*root, name.strip_prefix(root)?.strip_prefix('-')?)))
                .max_by_key(|(root, _)| root.len())?
        };
        if value.is_empty() {
            return None;
        }
        let rank = order(root)? as u16;
        match root {
            "group" | "peer" | "not" | "has" | "in" => {
                let named = matches!(root, "group" | "peer");
                let inner = match modifier {
                    Some(modifier) if !named => format!("{value}/{modifier}"),
                    _ => value.to_owned(),
                };
                let (inner_rank, inner) = self.variant(&inner, depth + 1)?;
                if inner.never || inner.relative && !matches!(root, "has" | "in") {
                    return None;
                }
                let branches = if root == "not" {
                    let [(at, selector)] = <[_; 1]>::try_from(inner.branches).ok()?;
                    if at.len() > 1 {
                        return None;
                    }
                    let mut branches = Vec::new();
                    if let Some(selector) = selector {
                        if selector.contains("::") {
                            return None;
                        }
                        let list: Vec<_> = split(&selector, ',')
                            .into_iter()
                            .map(|part| universal(part.trim(), true))
                            .collect();
                        branches.push((Vec::new(), Some(format!("&:not({})", list.join(", ")))));
                    }
                    if let Some(rule) = at.first() {
                        branches.push((vec![negate(rule)?], None));
                    }
                    branches
                } else {
                    let marker = match modifier {
                        Some(name) if named && named_value(name) => {
                            format!(":where(.{root}\\/{})", &escape(name)[1..])
                        }
                        Some(_) if named => return None,
                        _ => format!(":where(.{root})"),
                    };
                    let mut branches = Vec::new();
                    for (at, selector) in inner.branches {
                        let selector = selector?;
                        let selector = match root {
                            "has" => format!("&:has({})", universal(&selector, true)),
                            "in" => format!(":where({}) &", universal(&selector, false)),
                            _ => {
                                let list = selector.replace('&', &marker);
                                let list = if split(&list, ',').len() > 1 {
                                    format!(":is({list})")
                                } else {
                                    list
                                };
                                let target = if root == "group" { "*" } else { "~ *" };
                                format!("&:is({list} {target})")
                            }
                        };
                        branches.push((at, Some(selector)));
                    }
                    branches
                };
                if branches.is_empty() {
                    return None;
                }
                Some((
                    (
                        rank,
                        format!(
                            "{:05}{}{}",
                            inner_rank.0,
                            inner_rank.1,
                            modifier.map_or(String::new(), |name| format!("\0{name}"))
                        ),
                    ),
                    Variant {
                        branches,
                        ..Variant::default()
                    },
                ))
            }
            _ if modifier.is_some() && !root.starts_with('@') => None,
            "aria" | "data" => {
                let (key, selector) = if value.starts_with('[') {
                    let value = self.arbitrary(value)?;
                    let selector = format!("&[{root}-{}]", quote(&value));
                    (format!("1{value}"), selector)
                } else if !named_value(value) {
                    return None;
                } else if root == "aria" {
                    (format!("0{value}"), format!("&[aria-{value}=\"true\"]"))
                } else {
                    (format!("0{value}"), format!("&[data-{value}]"))
                };
                Some(((rank, key), template(&selector)))
            }
            "nth" | "nth-last" | "nth-of-type" | "nth-last-of-type" => {
                let (kind, argument) = if value.starts_with('[') {
                    ('1', self.arbitrary(value)?)
                } else {
                    let number = value.parse::<u32>().ok().filter(|_| integer(value))?;
                    ('0', number.to_string())
                };
                let pseudo = match root {
                    "nth" => "nth-child",
                    "nth-last" => "nth-last-child",
                    _ => root,
                };
                Some((
                    (rank, format!("{kind}{argument}")),
                    template(&format!("&:{pseudo}({argument})")),
                ))
            }
            "supports" => {
                if value == "bg-glass" {
                    return Some((
                        (rank, "0bg-glass".into()),
                        template("@media (-macos-glass)"),
                    ));
                }
                let (kind, raw) = if value.starts_with(['[', '(']) {
                    ('1', self.arbitrary(value)?)
                } else if named_value(value) {
                    ('0', value.to_owned())
                } else {
                    return None;
                };
                let name_end = raw
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(raw.len());
                let condition = if raw[name_end..].trim_start().starts_with('(') {
                    raw.clone()
                } else {
                    let raw = if raw.contains(':') {
                        raw.clone()
                    } else {
                        format!("{raw}: var(--tw)")
                    };
                    if raw.starts_with('(') && raw.ends_with(')') {
                        raw
                    } else {
                        format!("({raw})")
                    }
                };
                Some((
                    (rank, format!("{kind}{raw}")),
                    Variant {
                        branches: vec![(vec![format!("@supports {condition}")], None)],
                        ..Variant::default()
                    },
                ))
            }
            "min" | "max" => {
                let width = match self.arbitrary(value) {
                    Some(raw) => raw,
                    None => self.raw(&format!("--breakpoint-{value}"))?,
                };
                self.media(if root == "min" { ">=" } else { "<" }, &width, root)
            }
            _ => {
                let width = match self.arbitrary(value) {
                    Some(raw) => raw,
                    None => self.raw(&format!("--container-{value}"))?,
                };
                let name = match modifier {
                    Some(name) if named_value(name) => format!("{name} "),
                    Some(_) => return None,
                    None => String::new(),
                };
                let operator = if root == "@max" { "<" } else { ">=" };
                let (rank, _) = self.media(operator, &width, root)?;
                Some((
                    rank,
                    Variant {
                        branches: vec![(
                            vec![format!("@container {name}(width {operator} {width})")],
                            None,
                        )],
                        ..Variant::default()
                    },
                ))
            }
        }
    }

    pub(super) fn media(&self, operator: &str, width: &str, root: &str) -> Option<(Rank, Variant)> {
        // Tailwind groups breakpoints by unit (or function name) and only then by number, so `px` values sort
        // before `rem` values.
        let bucket = match width.find('(') {
            Some(end) => width[..end].to_owned(),
            None => width.replace(|c: char| c.is_ascii_digit() || c == '.', ""),
        };
        let sign = usize::from(width.starts_with(['+', '-']));
        let end = width[sign..]
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map_or(width.len(), |end| sign + end);
        let key = match width[..end].parse::<f64>() {
            Ok(value) => {
                let value = if operator == "<" { -value } else { value } + 0.;
                let bits = value.to_bits();
                let bits = if value < 0. { !bits } else { bits | 1 << 63 };
                format!("{bucket}\0{bits:016x}")
            }
            Err(_) => format!("{bucket}\0\u{1}{width}"),
        };
        Some((
            (
                VARIANTS.iter().position(|(name, _)| *name == root)? as u16,
                key,
            ),
            Variant {
                branches: vec![(vec![format!("@media (width {operator} {width})")], None)],
                ..Variant::default()
            },
        ))
    }
}

pub(super) fn template(source: &str) -> Variant {
    let mut variant = Variant::default();
    if source.contains("@slot") {
        slots(source, &[], None, &mut variant.branches, 0);
    } else if source.starts_with('@') {
        variant.branches.push((vec![source.to_owned()], None));
    } else {
        variant.branches.push((Vec::new(), Some(source.to_owned())));
    }
    variant.never = variant.branches.iter().any(|(at, selector)| {
        selector
            .as_ref()
            .is_some_and(|selector| selector.contains("::"))
            || at.iter().any(|rule| {
                !["@media", "@supports", "@container"]
                    .iter()
                    .any(|name| rule.starts_with(name))
            })
    });
    variant
}

pub(super) fn slots(
    source: &str,
    at: &[String],
    selector: Option<&str>,
    out: &mut Vec<(Vec<String>, Option<String>)>,
    depth: u8,
) {
    if depth > 8 || out.len() > 8 {
        return;
    }
    for (prelude, block) in statements(source) {
        match block {
            None if prelude == "@slot" => out.push((at.to_vec(), selector.map(str::to_owned))),
            None => {}
            Some(inner) if prelude.starts_with('@') => {
                let mut at = at.to_vec();
                at.push(prelude.to_owned());
                slots(inner, &at, selector, out, depth + 1);
            }
            Some(inner) => {
                let nested = match selector {
                    Some(outer) if prelude.contains('&') => prelude.replace('&', outer),
                    Some(outer) => format!("{outer} {prelude}"),
                    None if prelude.contains('&') => prelude.to_owned(),
                    None => format!("& {prelude}"),
                };
                slots(inner, at, Some(&nested), out, depth + 1);
            }
        }
    }
}
