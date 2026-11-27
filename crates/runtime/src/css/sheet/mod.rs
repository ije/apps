pub(crate) mod media;
pub(crate) mod parser;

use crate::css::{
    properties::Prop, properties::Style, selector::Selector, selector::SelectorList, value::anchor,
    value::grammar::computed, value::grammar::fold, value::grammar::supports,
    value::shorthand::BORDER_IMAGE, value::shorthand::BORDER_IMAGE_INITIAL,
    value::shorthand::expand,
};
use std::rc::Rc;
// The last field is the position of the declaration in its block, which breaks ties between properties.
pub(crate) type Declared = (Prop, Rc<str>, bool, u16);

#[derive(Clone, Debug, Default, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Declarations(pub(crate) Vec<Declaration>);

#[derive(Clone, Debug, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Declaration {
    Property(String, String),
    Apply(Vec<String>),
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Stylesheet(pub(crate) Vec<Rc<Rule>>);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) enum Rule {
    Style(SelectorList, Vec<Rule>),
    Media(Vec<MediaQuery>, Vec<Rule>),
    Layer(Vec<String>, Vec<Rule>),
    Supports(String, bool, Vec<Rule>),
    Declarations(Declarations),
    Theme(String, Vec<Rule>),
    // A `@custom-variant` definition: a selector template, or a block with `@slot`.
    CustomVariant(String, String),
    // A `@variant` block: the name and the body without braces.
    VariantBlock(String, String),
    Utility(String, String),
    FontFace(Declarations),
    Tailwind(String),
    Keyframes(String, Vec<(f32, Declarations)>),
    Container(Option<String>, bool, MediaQuery, Vec<Rule>),
    Property(String, String, bool, Option<String>),
    Starting(Vec<Rule>),
    PositionTry(String, Declarations),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) enum MediaQuery {
    All,
    Feature(String, String, String),
    Unknown(String),
    Not(Box<MediaQuery>),
    And(Vec<MediaQuery>),
    Or(Vec<MediaQuery>),
    Typed(String, Box<MediaQuery>),
    // A range with the value first, and the text that it keeps.
    Range(String, Box<MediaQuery>),
    // A container `style()` query of a custom property, and its value or an empty text.
    Style(String, String),
}

pub(crate) fn declare(source: &str) -> Vec<[String; 3]> {
    let mut result = Vec::new();
    for declaration in Declarations::parse(source).0 {
        let Declaration::Property(name, value) = declaration else {
            continue;
        };
        let (value, important) = match value.strip_suffix("!important") {
            Some(value) => (value.trim_end(), "important"),
            None => (value.as_str(), ""),
        };
        let known = name.starts_with("--") || Prop::find(&name).is_some_and(Prop::is_known);
        if !known || supports(&name, value) == Some(false) {
            continue;
        }
        let mut expanded = Vec::new();
        // Scripts read these shorthands back by name, and `style` has no serializer for them.
        if value.contains("var(")
            || matches!(
                name.as_str(),
                "columns"
                    | "column-rule"
                    | "grid-row"
                    | "grid-column"
                    | "grid-area"
                    | "grid-template"
                    | "grid"
                    | "mask"
                    | "offset"
            )
            || !expand(&name, value, &mut expanded)
        {
            expanded = vec![(name.clone(), value.to_owned())];
        } else if name == "border" {
            // Like Chrome, scripts read `border` as a reset of all of `border-image`, though the cascade resets
            // only its source.
            let wide = matches!(
                value,
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            );
            expanded.truncate(12);
            expanded.extend(BORDER_IMAGE.iter().zip(BORDER_IMAGE_INITIAL).map(
                |(name, initial)| {
                    (
                        (*name).to_owned(),
                        if wide { value } else { initial }.to_owned(),
                    )
                },
            ));
        }
        result.extend(
            expanded
                .into_iter()
                .map(|(name, value)| [name, value, important.to_owned()]),
        );
    }
    result
}

impl Rule {
    fn bind_parent(&mut self, parent: Option<&SelectorList>) -> anyhow::Result<()> {
        match self {
            Self::Style(selectors, children) => {
                for selector in Rc::make_mut(selectors) {
                    selector.bind_parent(parent)?;
                }
                for child in children {
                    child.bind_parent(Some(selectors))?;
                }
            }
            Self::Media(_, children)
            | Self::Layer(_, children)
            | Self::Supports(_, _, children)
            | Self::Starting(children)
            | Self::Container(.., children) => {
                for child in children {
                    child.bind_parent(parent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn plain(&self) -> bool {
        match self {
            Self::Layer(..)
            | Self::Keyframes(..)
            | Self::Property(..)
            | Self::Theme(..)
            | Self::PositionTry(..) => false,
            Self::Style(_, children)
            | Self::Media(_, children)
            | Self::Supports(_, _, children)
            | Self::Starting(children)
            | Self::Container(.., children) => children.iter().all(Self::plain),
            _ => true,
        }
    }

    pub(crate) fn source(&self) -> String {
        match self {
            Self::Declarations(values) => values.source(),
            Self::Layer(names, children) => {
                if children.is_empty() {
                    format!("@layer {};", names.join(","))
                } else {
                    format!(
                        "@layer {}{{{}}}",
                        names.join(","),
                        children.iter().map(Self::source).collect::<String>()
                    )
                }
            }
            Self::Supports(condition, _, children) => format!(
                "@supports {condition}{{{}}}",
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Starting(children) => format!(
                "@starting-style{{{}}}",
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Container(name, _, query, children) => format!(
                "@container {}{}{{{}}}",
                name.as_ref()
                    .map_or(String::new(), |name| format!("{name} ")),
                query.source(),
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Theme(options, children) => format!(
                "@theme {options}{{{}}}",
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Utility(name, body) => format!("@utility {name}{{{body}}}"),
            Self::PositionTry(name, values) => {
                format!("@position-try {name}{{{}}}", values.source())
            }
            Self::FontFace(values) => format!("@font-face{{{}}}", values.source()),
            Self::VariantBlock(name, body) => format!("@variant {name}{{{body}}}"),
            Self::CustomVariant(name, body) if body.contains("@slot") => {
                format!("@custom-variant {name}{{{body}}}")
            }
            Self::CustomVariant(name, template) => format!("@custom-variant {name} ({template});"),
            Self::Property(name, syntax, inherits, initial) => format!(
                "@property {name}{{syntax:{};inherits:{inherits};{}}}",
                serde_json::to_string(syntax).unwrap_or_default(),
                initial
                    .as_ref()
                    .map_or(String::new(), |value| format!("initial-value:{value}"))
            ),
            Self::Tailwind(text) => format!("@{text};"),
            Self::Keyframes(name, frames) => format!(
                "@keyframes {name}{{{}}}",
                frames
                    .iter()
                    .map(|(at, values)| format!("{}%{{{}}}", at * 100., values.source()))
                    .collect::<String>()
            ),
            Self::Style(selectors, children) => format!(
                "{}{{{}}}",
                selectors
                    .iter()
                    .map(Selector::source)
                    .collect::<Vec<_>>()
                    .join(","),
                children.iter().map(Self::source).collect::<String>(),
            ),
            Self::Media(queries, children) => format!(
                "@media {}{{{}}}",
                queries
                    .iter()
                    .map(MediaQuery::source)
                    .collect::<Vec<_>>()
                    .join(","),
                children.iter().map(Self::source).collect::<String>(),
            ),
        }
    }
}

/// An `@font-face` rule.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct FontFace {
    pub(crate) family: String,
    pub(crate) src: String,
    pub(crate) range: Option<String>,
    pub(crate) weight: Option<String>,
    pub(crate) style: Option<String>,
    pub(crate) stretch: Option<String>,
}

pub(crate) fn declarations(source: &str) -> Style {
    Declarations::parse(source).resolve()
}

impl Declarations {
    pub(crate) fn resolve(&self) -> Style {
        self.resolved(false)
            .into_iter()
            .map(|(prop, value, important, _)| match important {
                true => (prop, format!("{value} !important").into()),
                false => (prop, value),
            })
            .collect()
    }

    // Drop unknown inline properties before interning: scripts can create unlimited names, and interned names
    // never free.
    pub(crate) fn resolved(&self, inline: bool) -> Vec<Declared> {
        let mut result = Vec::new();
        for (position, declaration) in self.0.iter().enumerate() {
            let Declaration::Property(key, value) = declaration else {
                continue;
            };
            let important = value.ends_with("!important");
            let plain = value.trim_end_matches("!important").trim();
            let mut expanded = Vec::new();
            if plain.contains("var(") && expand(key, "initial", &mut expanded) {
                for (_, value) in &mut expanded {
                    *value = format!("\u{1}{key}\u{1}{plain}");
                }
            } else if !expand(key, plain, &mut expanded) {
                expanded.push((key.clone(), plain.to_owned()));
            }
            for (key, value) in expanded {
                let value = fold(&key, &value).unwrap_or(value);
                let prop = match inline && !key.starts_with("--") {
                    true => match Prop::find(&key) {
                        Some(prop) => prop,
                        None => continue,
                    },
                    false => Prop::new(&key),
                };
                result.push((prop, value.into(), important, position.min(65535) as u16));
            }
        }
        // Keep one declaration per property: the last important one, or else the last one.
        result.sort_unstable_by_key(|entry| (entry.0, !entry.2, std::cmp::Reverse(entry.3)));
        result.dedup_by_key(|entry| entry.0);
        result
    }

    /// Keeps what an `@position-try` rule takes.
    pub(crate) fn position_try(mut values: Self) -> Self {
        values.0.retain(|declaration| match declaration {
            Declaration::Property(name, value) => {
                !value.ends_with("!important")
                    && Prop::find(name).is_some_and(|prop| anchor::DESCRIPTORS.contains(prop))
            }
            Declaration::Apply(_) => false,
        });
        values
    }

    pub(crate) fn source(&self) -> String {
        self.0
            .iter()
            .map(|declaration| match declaration {
                Declaration::Property(key, value) => format!("{key}:{value};"),
                Declaration::Apply(classes) => format!("@apply {};", classes.join(" ")),
            })
            .collect()
    }
}

impl Rule {
    /// The syntax, the inheritance and the computed initial value of an `@property` rule.
    pub(crate) fn property(&self) -> Option<(Rc<str>, bool, Option<Rc<str>>)> {
        let Self::Property(_, syntax, inherits, initial) = self else {
            return None;
        };
        let initial = initial.as_deref().map(|value| match syntax.trim() {
            "*" => value.into(),
            _ => computed(syntax, value, None).0.into(),
        });
        Some((syntax.as_str().into(), *inherits, initial))
    }

    pub(crate) fn cssom(&self) -> serde_json::Value {
        use serde_json::json;
        let declarations = |values: &Declarations| {
            values
                .0
                .iter()
                .filter_map(|declaration| {
                    let Declaration::Property(name, value) = declaration else {
                        return None;
                    };
                    let (value, important) = match value.strip_suffix("!important") {
                        Some(value) => (value.trim_end(), "important"),
                        None => (value.as_str(), ""),
                    };
                    Some(json!([name, value, important]))
                })
                .collect::<Vec<_>>()
        };
        let list = |children: &[Rule]| {
            children
                .iter()
                .filter(|rule| !matches!(rule, Self::Declarations(_)))
                .map(Self::cssom)
                .collect::<Vec<_>>()
        };
        match self {
            Self::Style(selectors, children) => json!([
                "style",
                selectors
                    .iter()
                    .map(Selector::source)
                    .collect::<Vec<_>>()
                    .join(", "),
                children
                    .iter()
                    .flat_map(|rule| match rule {
                        Self::Declarations(values) => declarations(values),
                        _ => Vec::new(),
                    })
                    .collect::<Vec<_>>(),
                list(children),
            ]),
            Self::Media(queries, children) => json!([
                "media",
                queries.iter().map(MediaQuery::source).collect::<Vec<_>>(),
                list(children)
            ]),
            Self::Supports(condition, _, children) => {
                json!(["supports", condition.trim(), list(children)])
            }
            Self::Starting(children) => json!(["starting-style", "", list(children)]),
            Self::Layer(names, children) if children.is_empty() => json!(["layers", names]),
            Self::Layer(names, children) => json!(["layer", names.join(","), list(children)]),
            Self::Container(name, _, query, children) => json!([
                "container",
                name.as_ref()
                    .map_or_else(String::new, |name| format!("{name} "))
                    + &query.source(),
                list(children)
            ]),
            Self::Keyframes(name, frames) => json!([
                "keyframes",
                name,
                frames
                    .iter()
                    .map(|(at, values)| json!([format!("{}%", at * 100.), declarations(values)]))
                    .collect::<Vec<_>>()
            ]),
            Self::PositionTry(name, values) => {
                json!(["position-try", name, declarations(values)])
            }
            Self::FontFace(values) => json!(["font-face", declarations(values)]),
            Self::Property(name, syntax, inherits, initial) => {
                json!(["property", name, syntax, inherits, initial])
            }
            Self::Tailwind(text) if text.starts_with("import") => {
                json!(["import", text.split('"').nth(1)])
            }
            Self::Tailwind(_) => json!(["other", self.source()]),
            Self::Declarations(_)
            | Self::Theme(..)
            | Self::CustomVariant(..)
            | Self::VariantBlock(..)
            | Self::Utility(..) => {
                json!(["other", self.source()])
            }
        }
    }
}

pub(crate) enum Edit<'a> {
    Insert(&'a str),
    Delete,
    Declarations(&'a str),
    Selector(&'a str),
    AppendKeyframe(&'a str),
    DeleteKeyframe(usize),
    Replace(&'a str),
}

impl Stylesheet {
    pub(crate) fn cssom(&self) -> serde_json::Value {
        serde_json::Value::Array(self.0.iter().map(|rule| rule.cssom()).collect())
    }

    pub(crate) fn property(&self) -> Option<(Rc<str>, bool, Option<Rc<str>>)> {
        let [rule] = self.0.as_slice() else {
            return None;
        };
        rule.property()
    }

    pub(crate) fn edit(
        &mut self,
        path: &[usize],
        edit: Edit,
    ) -> Result<serde_json::Value, &'static str> {
        let (&last, path) = path.split_last().ok_or("IndexSizeError")?;
        let Some((&first, path)) = path.split_first() else {
            return edit_rules(&mut self.0, last, edit, None, Rc::make_mut);
        };
        let mut parent = None;
        let index = position(&self.0, first).ok_or("IndexSizeError")?;
        let mut rules = children(Rc::make_mut(&mut self.0[index]), &mut parent)?;
        for &index in path {
            let index = position(rules, index).ok_or("IndexSizeError")?;
            rules = children(&mut rules[index], &mut parent)?;
        }
        edit_rules(rules, last, edit, parent, |rule| rule)
    }

    pub(crate) fn font_faces(&self) -> Vec<FontFace> {
        let mut faces = Vec::new();
        let mut pending: Vec<&Rule> = self.0.iter().rev().map(|rule| &**rule).collect();
        while let Some(rule) = pending.pop() {
            let values = match rule {
                Rule::FontFace(values) => values.resolve(),
                Rule::Layer(_, children)
                | Rule::Media(_, children)
                | Rule::Supports(_, true, children) => {
                    pending.extend(children.iter().rev());
                    continue;
                }
                _ => continue,
            };
            let (Some(family), Some(src)) =
                (values.get(Prop::FONT_FAMILY), values.get(Prop::new("src")))
            else {
                continue;
            };
            let descriptor = |prop| values.get(prop).map(str::to_owned);
            faces.push(FontFace {
                family: family.trim_matches(['\'', '"']).to_owned(),
                src: src.to_owned(),
                // Declarations keep `U+0400-04FF` as tokens joined by `/**/`.
                range: values
                    .get(Prop::new("unicode-range"))
                    .map(|range| range.replace("/**/", "")),
                weight: descriptor(Prop::FONT_WEIGHT),
                style: descriptor(Prop::FONT_STYLE),
                stretch: descriptor(Prop::FONT_STRETCH),
            });
        }
        faces
    }

    pub(crate) fn matches_media(&self, width: f32, height: f32, theme: u32) -> bool {
        let [rule] = self.0.as_slice() else {
            return false;
        };
        matches!(&**rule, Rule::Media(queries, _)
            if MediaQuery::list(queries, [width, height], theme & 32 != 0, theme))
    }

    pub(crate) fn bind_parents(&mut self) -> anyhow::Result<()> {
        for rule in &mut self.0 {
            Rc::make_mut(rule).bind_parent(None)?;
        }
        Ok(())
    }

    pub(crate) fn source(&self) -> String {
        self.0.iter().map(|rule| rule.source()).collect()
    }
}

fn position<R: std::borrow::Borrow<Rule>>(rules: &[R], n: usize) -> Option<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| !matches!((*rule).borrow(), Rule::Declarations(_)))
        .nth(n)
        .map(|(index, _)| index)
}

fn children<'a>(
    rule: &'a mut Rule,
    parent: &mut Option<SelectorList>,
) -> Result<&'a mut Vec<Rule>, &'static str> {
    match rule {
        Rule::Style(selectors, children) => {
            *parent = Some(selectors.clone());
            Ok(children)
        }
        Rule::Media(_, children)
        | Rule::Supports(_, _, children)
        | Rule::Layer(_, children)
        | Rule::Container(.., children) => Ok(children),
        _ => Err("HierarchyRequestError"),
    }
}

fn edit_rules<R: std::borrow::Borrow<Rule> + From<Rule>>(
    rules: &mut Vec<R>,
    last: usize,
    edit: Edit,
    parent: Option<SelectorList>,
    rule_mut: fn(&mut R) -> &mut Rule,
) -> Result<serde_json::Value, &'static str> {
    let at = position(rules, last);
    let parse = |text: &str| {
        let mut parsed = match &parent {
            Some(parent) => Rule::parse_nested(text, parent),
            None => Stylesheet::parse(text)
                .0
                .into_iter()
                .map(Rc::unwrap_or_clone)
                .collect(),
        };
        if parsed.len() != 1 || matches!(parsed[0], Rule::Declarations(_)) {
            return Err("SyntaxError");
        }
        Ok(parsed.remove(0))
    };
    if let Edit::Insert(text) = edit {
        let at = match at {
            Some(at) => at,
            None if last == 0 || position(rules, last - 1).is_some() => rules.len(),
            None => return Err("IndexSizeError"),
        };
        let rule = parse(text)?;
        let tree = rule.cssom();
        rules.insert(at, R::from(rule));
        return Ok(tree);
    }
    let index = at.ok_or("IndexSizeError")?;
    if matches!(edit, Edit::Delete) {
        rules.remove(index);
        return Ok(serde_json::Value::Null);
    }
    match (edit, rule_mut(&mut rules[index])) {
        (Edit::Declarations(text), Rule::Style(_, children)) => {
            children.retain(|rule| !matches!(rule, Rule::Declarations(_)));
            children.insert(0, Rule::Declarations(Declarations::parse(text)));
        }
        (Edit::Declarations(text), Rule::FontFace(values)) => {
            *values = Declarations::descriptors(text)
        }
        (Edit::Declarations(text), Rule::PositionTry(_, values)) => {
            *values = Declarations::position_try(Declarations::parse(text));
        }
        (Edit::Selector(text), Rule::Style(selectors, children)) => {
            let parsed = Selector::parse(text, parent.as_ref());
            if parsed.is_empty() {
                return Err("SyntaxError");
            }
            *selectors = Rc::new(parsed);
            for child in children {
                let _ = child.bind_parent(Some(selectors));
            }
            return Ok(selectors
                .iter()
                .map(Selector::source)
                .collect::<Vec<_>>()
                .join(", ")
                .into());
        }
        (Edit::Replace(text), rule) => {
            let mut new = parse(text)?;
            if std::mem::discriminant(&new) != std::mem::discriminant(rule) {
                return Err("SyntaxError");
            }
            if let (Ok(old), Ok(added)) = (children(rule, &mut None), children(&mut new, &mut None))
            {
                std::mem::swap(old, added);
            }
            *rule = new;
        }
        // As in CSSOM, an append that doesn't parse does nothing.
        (Edit::AppendKeyframe(text), Rule::Keyframes(name, frames)) => {
            let parsed = Stylesheet::parse(&format!("@keyframes {name}{{{text}}}"));
            if let Some(Rule::Keyframes(_, mut added)) =
                parsed.0.into_iter().next().map(Rc::unwrap_or_clone)
                && !added.is_empty()
            {
                let tree = Rule::Keyframes(String::new(), added.clone()).cssom()[2].clone();
                frames.append(&mut added);
                return Ok(tree);
            }
        }
        (Edit::DeleteKeyframe(index), Rule::Keyframes(_, frames)) => {
            if index >= frames.len() {
                return Err("IndexSizeError");
            }
            frames.remove(index);
        }
        _ => return Err("InvalidModificationError"),
    }
    Ok(serde_json::Value::Null)
}
