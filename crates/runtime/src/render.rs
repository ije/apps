use crate::html::{Document, Selector, SelectorCache, SelectorList};
use std::collections::BTreeMap;
use std::{borrow::Cow, cmp::Reverse, collections::HashMap, rc::Rc};

#[path = "animation.rs"]
mod animation;
#[path = "color.rs"]
mod colors;
#[path = "css.rs"]
mod css;
pub(crate) use animation::Animation;
pub use colors::color;
pub(crate) use css::{FontUnits, Lengths};

pub type Style = BTreeMap<String, String>;
#[derive(Clone)]
struct StyleRef(Rc<Style>);

impl PartialEq for StyleRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for StyleRef {}
impl std::hash::Hash for StyleRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Rc::as_ptr(&self.0), state);
    }
}

#[derive(Hash, PartialEq, Eq)]
struct StyleKey {
    scope: usize,
    tag: String,
    classes: Option<String>,
    inline: Option<String>,
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
    host: Option<Style>,
    selection: Option<Style>,
}

struct CachedNode {
    parent: Option<StyleRef>,
    font: [u32; 6],
    root: [u32; 6],
    host: Option<Style>,
    selection: Option<Style>,
    value: Rc<CachedStyle>,
}

impl CachedNode {
    fn new(key: &StyleKey, value: Rc<CachedStyle>) -> Self {
        Self {
            parent: key.parent.clone(),
            font: key.font,
            root: key.root,
            host: key.host.clone(),
            selection: key.selection.clone(),
            value,
        }
    }
}

struct CachedStyle {
    style: Rc<Style>,
    lengths: Lengths,
    pseudos: Option<Rc<[Style; 2]>>,
    selection: Option<Style>,
    animations: Option<Vec<Animation>>,
    host: Style,
}

struct StyleScope {
    restricted: bool,
    sheets: Vec<Rc<Stylesheet>>,
    rules: Vec<(SelectorList, Style, i32)>,
    pseudos: Vec<(usize, Selector, usize)>,
    elements: Vec<Vec<usize>>,
    tailwind: bool,
    theme: Style,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
    matches: SelectorCache,
    nodes: Vec<Option<MatchedRules>>,
}

struct MatchedRules {
    checked: std::cell::Cell<u64>,
    cached: Option<CachedNode>,
    dependencies: Vec<(usize, u64)>,
    rules: Vec<(usize, [usize; 3])>,
    pseudos: Vec<usize>,
}

#[derive(Default)]
pub(crate) struct StyleCache {
    viewport: [f32; 2],
    metrics: bool,
    reduced_motion: bool,
    native_theme: u32,
    scopes: Vec<StyleScope>,
    values: HashMap<StyleKey, Rc<CachedStyle>>,
}
type CascadeRank = (bool, i32, [usize; 4], usize);

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Declarations(Vec<Declaration>);

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
enum Declaration {
    Property(String, String),
    Apply(Vec<String>),
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Stylesheet(Vec<Rule>);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
enum Rule {
    Style(SelectorList, Vec<Rule>),
    Media(Vec<Vec<(MediaFeature, String)>>, Vec<Rule>),
    Layer(Vec<String>, Vec<Rule>),
    Supports(String, Vec<Rule>),
    Declarations(Declarations),
    Theme(Declarations),
    Tailwind,
    Keyframes(String, Vec<(f32, Declarations)>),
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
enum MediaFeature {
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
    Pointer,
    Width,
    Height,
    ReducedMotion,
    ColorScheme,
    Contrast,
    InvertedColors,
    ReducedTransparency,
    ForcedColors,
}

pub(crate) fn reduced_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn apps_reduce_motion() -> bool;
        }
        unsafe { apps_reduce_motion() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

impl MediaFeature {
    fn matches(
        self,
        value: &str,
        width: f32,
        height: f32,
        reduced_motion: bool,
        theme: u32,
    ) -> bool {
        match self {
            Self::ColorScheme => return value == if theme & 1 != 0 { "dark" } else { "light" },
            Self::Contrast => {
                return value
                    == if theme & 2 != 0 {
                        "more"
                    } else {
                        "no-preference"
                    };
            }
            Self::InvertedColors => {
                return value == if theme & 4 != 0 { "inverted" } else { "none" };
            }
            Self::ReducedTransparency => {
                return value
                    == if theme & 8 != 0 {
                        "reduce"
                    } else {
                        "no-preference"
                    };
            }
            Self::ForcedColors => return value == "none",
            _ => {}
        }
        if matches!(self, Self::Pointer) {
            return value == "fine";
        }
        if matches!(self, Self::ReducedMotion) {
            return value
                == if reduced_motion {
                    "reduce"
                } else {
                    "no-preference"
                };
        }
        let Some(boundary) = Lengths::new(16., width, height).resolve(value, width) else {
            return false;
        };
        match self {
            Self::MinWidth => width >= boundary,
            Self::MaxWidth => width <= boundary,
            Self::MinHeight => height >= boundary,
            Self::MaxHeight => height <= boundary,
            Self::Width => width == boundary,
            Self::Height => height == boundary,
            _ => unreachable!(),
        }
    }
}

struct Collect<'a> {
    width: f32,
    height: f32,
    reduced_motion: bool,
    native_theme: u32,
    rules: Vec<(&'a SelectorList, &'a Declarations, i32)>,
    layers: Vec<String>,
    tailwind: bool,
    theme: Style,
    keyframes: HashMap<String, Vec<(f32, Declarations)>>,
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
            Self::Media(_, children) | Self::Layer(_, children) | Self::Supports(_, children) => {
                for child in children {
                    child.bind_parent(parent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn collect<'a>(
        &'a self,
        parent: Option<&'a SelectorList>,
        layer: &str,
        collected: &mut Collect<'a>,
    ) {
        match self {
            Self::Declarations(values) => {
                if let Some(parent) = parent {
                    collected.rules.push((
                        parent,
                        values,
                        collected
                            .layers
                            .iter()
                            .position(|name| name == layer)
                            .map_or(i32::MAX, |n| n as i32),
                    ));
                }
            }
            Self::Theme(values) => {
                for (key, value) in values.resolve(None, collected.width) {
                    if key.starts_with("--") {
                        collected.theme.insert(key, value);
                    }
                }
            }
            Self::Tailwind => collected.tailwind = true,
            Self::Keyframes(name, frames) => {
                collected.keyframes.insert(name.clone(), frames.clone());
            }
            Self::Style(selectors, children) => {
                for rule in children {
                    rule.collect(Some(selectors), layer, collected);
                }
            }
            Self::Layer(names, children) => {
                let names = if names.is_empty() {
                    vec![format!("__anonymous_{}", collected.layers.len())]
                } else {
                    names.clone()
                };
                for name in names {
                    let name = if layer.is_empty() {
                        name
                    } else {
                        format!("{layer}.{name}")
                    };
                    if !collected.layers.contains(&name) {
                        collected.layers.push(name.clone());
                    }
                    for rule in children {
                        rule.collect(parent, &name, collected);
                    }
                }
            }
            Self::Supports(condition, children) => {
                let condition = condition.trim().trim_matches(['(', ')']);
                let supported =
                    condition
                        .split_once(':')
                        .is_some_and(|(name, value)| match name.trim() {
                            "display" => matches!(value.trim(), "grid" | "flex" | "block"),
                            "width" => {
                                value.trim() == "1cqi"
                                    || Lengths::new(16., collected.width, collected.height)
                                        .resolve(value.trim(), collected.width)
                                        .is_some()
                            }
                            _ => false,
                        });
                if supported {
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
            Self::Media(queries, children) => {
                if queries.iter().any(|query| {
                    query.iter().all(|(feature, value)| {
                        feature.matches(
                            value,
                            collected.width,
                            collected.height,
                            collected.reduced_motion,
                            collected.native_theme,
                        )
                    })
                }) {
                    for rule in children {
                        rule.collect(parent, layer, collected);
                    }
                }
            }
        }
    }

    fn source(&self) -> String {
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
            Self::Supports(condition, children) => format!(
                "@supports {condition}{{{}}}",
                children.iter().map(Self::source).collect::<String>()
            ),
            Self::Theme(values) => format!("@theme{{{}}}", values.source()),
            Self::Tailwind => "@import 'tailwindcss';".into(),
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
                    .map(|query| {
                        if query.is_empty() {
                            return "all".into();
                        }
                        query
                            .iter()
                            .map(|(feature, value)| {
                                let key = match feature {
                                    MediaFeature::MinWidth => "min-width",
                                    MediaFeature::MaxWidth => "max-width",
                                    MediaFeature::MinHeight => "min-height",
                                    MediaFeature::MaxHeight => "max-height",
                                    MediaFeature::Pointer => "pointer",
                                    MediaFeature::Width => "width",
                                    MediaFeature::Height => "height",
                                    MediaFeature::ReducedMotion => "prefers-reduced-motion",
                                    MediaFeature::ColorScheme => "prefers-color-scheme",
                                    MediaFeature::Contrast => "prefers-contrast",
                                    MediaFeature::InvertedColors => "inverted-colors",
                                    MediaFeature::ReducedTransparency => {
                                        "prefers-reduced-transparency"
                                    }
                                    MediaFeature::ForcedColors => "forced-colors",
                                };
                                format!("({key}:{value})")
                            })
                            .collect::<Vec<_>>()
                            .join(" and ")
                    })
                    .collect::<Vec<_>>()
                    .join(","),
                children.iter().map(Self::source).collect::<String>(),
            ),
        }
    }
}

#[derive(Default)]
pub(crate) struct StyleContext {
    pub hosts: HashMap<usize, Style>,
    pub inherited: HashMap<usize, (Rc<Style>, Lengths)>,
    pub roots: HashMap<usize, (Rc<Style>, Lengths)>,
    pub required: Option<std::collections::HashSet<usize>>,
}

pub(crate) struct ComputedStyles {
    pub values: Vec<Rc<Style>>,
    pub lengths: Vec<Lengths>,
    pub pseudos: HashMap<usize, Rc<[Style; 2]>>,
    pub selections: HashMap<usize, Style>,
    pub user_select: Vec<(UserSelect, usize)>,
    pub animations: HashMap<usize, Vec<Animation>>,
    pub children: HashMap<usize, Vec<usize>>,
    pub host_declarations: Style,
}

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UserSelect {
    #[default]
    Text,
    None,
    All,
}

pub fn styles(doc: &Document, width: f32, height: f32) -> Vec<Style> {
    compute_styles(
        doc,
        width,
        height,
        None,
        &mut HashMap::new(),
        &mut StyleCache::default(),
        None,
    )
    .values
    .into_iter()
    .map(|style| (*style).clone())
    .collect()
}

pub(crate) fn compute_styles(
    doc: &Document,
    width: f32,
    height: f32,
    text_system: Option<&gpui::TextSystem>,
    cache: &mut HashMap<String, Rc<Stylesheet>>,
    style_cache: &mut StyleCache,
    context: Option<&StyleContext>,
) -> ComputedStyles {
    let reduced_motion = reduced_motion();
    let native_theme = crate::browser::theme_state();
    let mut collected = Collect {
        width,
        height,
        reduced_motion,
        native_theme,
        rules: Vec::new(),
        layers: Vec::new(),
        tailwind: false,
        theme: Style::new(),
        keyframes: HashMap::new(),
    };
    let mut order = Vec::new();
    let mut sheets = Vec::new();
    let mut pending = vec![0];
    while let Some(id) = pending.pop() {
        let node = &doc.nodes[id];
        order.push(id);
        pending.extend(node.children.iter().rev());
        if node.tag == "style" {
            sheets.push(match &node.stylesheet {
                Some(sheet) => sheet.clone(),
                None => {
                    let source = doc.text(id);
                    cache
                        .entry(source.clone())
                        .or_insert_with(|| Rc::new(Stylesheet::parse(&source)))
                        .clone()
                }
            });
        }
    }
    if style_cache.viewport != [width, height]
        || style_cache.metrics != text_system.is_some()
        || style_cache.reduced_motion != reduced_motion
        || style_cache.native_theme != native_theme
        || style_cache.values.len() > 4096
    {
        *style_cache = StyleCache {
            viewport: [width, height],
            metrics: text_system.is_some(),
            reduced_motion,
            native_theme,
            ..Default::default()
        };
    }
    let restricted = context.is_some_and(|context| context.required.is_some());
    let scope = style_cache
        .scopes
        .iter()
        .position(|cached| {
            cached.restricted == restricted
                && cached.sheets.len() == sheets.len()
                && cached
                    .sheets
                    .iter()
                    .zip(&sheets)
                    .all(|(a, b)| Rc::ptr_eq(a, b))
        })
        .unwrap_or_else(|| {
            for sheet in &sheets {
                for rule in &sheet.0 {
                    rule.collect(None, "", &mut collected);
                }
            }
            let Collect {
                rules,
                tailwind,
                theme,
                keyframes,
                ..
            } = collected;
            let rules: Vec<_> = rules
                .into_iter()
                .map(|(selector, body, layer)| {
                    (
                        selector.clone(),
                        body.resolve(tailwind.then_some(&theme), width),
                        layer,
                    )
                })
                .collect();
            let mut pseudos = Vec::new();
            let elements = rules
                .iter()
                .enumerate()
                .map(|(order, (selectors, ..))| {
                    selectors
                        .iter()
                        .enumerate()
                        .filter_map(|(index, selector)| {
                            if let Some((kind, selector)) = selector.without_pseudo_element() {
                                pseudos.push((kind, selector, order));
                                None
                            } else {
                                Some(index)
                            }
                        })
                        .collect()
                })
                .collect();
            style_cache.scopes.push(StyleScope {
                restricted,
                sheets: sheets.clone(),
                rules,
                pseudos,
                elements,
                tailwind,
                theme,
                keyframes,
                matches: SelectorCache::default(),
                nodes: Vec::new(),
            });
            style_cache.scopes.len() - 1
        });
    let scoped = &mut style_cache.scopes[scope];
    let rules = &scoped.rules;
    let tailwind = scoped.tailwind;
    let theme = &scoped.theme;
    let pseudo_rules: Vec<_> = scoped
        .pseudos
        .iter()
        .map(|(kind, selector, order)| {
            let (_, declarations, layer) = &rules[*order];
            (*kind, selector, declarations, *layer, *order)
        })
        .collect();
    scoped.nodes.resize_with(doc.nodes.len(), || None);
    let matches = &mut scoped.matches;
    matches.update(doc, &order);
    let element_selectors: Vec<Vec<_>> = scoped
        .elements
        .iter()
        .zip(rules)
        .map(|(indices, (selectors, ..))| indices.iter().map(|&index| &selectors[index]).collect())
        .collect();
    let mut buckets: HashMap<(u8, &str), Vec<usize>> = HashMap::new();
    let mut universal = Vec::new();
    for (index, selectors) in element_selectors.iter().enumerate() {
        for selector in selectors {
            match selector.key() {
                Some(key) => buckets.entry(key).or_default().push(index),
                None => universal.push(index),
            }
        }
    }
    let mut candidates: Vec<usize> = Vec::new();
    let mut utilities = HashMap::new();
    if tailwind {
        for &id in &order {
            if let Some(classes) = doc.nodes[id].attrs.get("class") {
                for class in classes.split_ascii_whitespace() {
                    utilities
                        .entry(class)
                        .or_insert_with(|| crate::tailwind::utility(class, width, &theme));
                }
            }
        }
    }
    let mut styles = vec![Rc::new(Style::new()); doc.nodes.len()];
    let mut lengths = vec![Lengths::new(16., width, height); doc.nodes.len()];
    let mut root = FontUnits::new(16.);
    let mut pseudos = HashMap::new();
    let mut selections: HashMap<usize, Style> = HashMap::new();
    let mut animations = HashMap::new();
    let mut font_metrics = HashMap::new();

    let mut host_declarations = Style::new();
    for id in order {
        if context
            .and_then(|context| context.required.as_ref())
            .is_some_and(|required| !required.contains(&id))
        {
            continue;
        }
        let node = &doc.nodes[id];
        if let Some((style, units)) = context.and_then(|context| context.roots.get(&id)) {
            styles[id] = style.clone();
            lengths[id] = *units;
            continue;
        }
        let inherited = context.and_then(|context| context.inherited.get(&id));
        let parent_style = inherited
            .map(|(style, _)| style)
            .or_else(|| node.parent.map(|parent| &styles[parent]));
        let parent = inherited.map_or_else(
            || node.parent.map_or(FontUnits::new(16.), |p| lengths[p].font),
            |(_, units)| units.font,
        );
        let font_key = [
            parent.em, parent.ch, parent.ex, parent.cap, parent.ic, parent.lh,
        ]
        .map(f32::to_bits);
        let root_font = [root.em, root.ch, root.ex, root.cap, root.ic, root.lh].map(f32::to_bits);
        let host = context.and_then(|context| context.hosts.get(&id));
        let selection = node.parent.and_then(|parent| selections.get(&parent));
        let mut key = None;
        let cached = scoped.nodes[id]
            .as_ref()
            .filter(|node| matches.valid_since(&node.dependencies, &node.checked))
            .and_then(|node| node.cached.as_ref())
            .filter(|cached| {
                cached.parent.as_ref().map(|parent| Rc::as_ptr(&parent.0))
                    == parent_style.map(Rc::as_ptr)
                    && cached.font == font_key
                    && cached.root == root_font
                    && cached.host.as_ref() == host
                    && cached.selection.as_ref() == selection
            })
            .map(|cached| cached.value.clone())
            .or_else(|| {
                let (matched, matched_pseudos) = if let Some(cached) = &scoped.nodes[id]
                    && matches.valid_since(&cached.dependencies, &cached.checked)
                {
                    (cached.rules.clone(), cached.pseudos.clone())
                } else {
                    matches.dependencies.clear();
                    matches.depend(id, 0);
                    candidates.clear();
                    if !node.tag.starts_with('#') {
                        candidates.extend(&universal);
                        let classes = node.attrs.get("class").into_iter();
                        for key in
                            std::iter::once((2, node.tag.as_str()))
                                .chain(node.attrs.get("id").map(|id| (0, id.as_str())))
                                .chain(classes.flat_map(|c| {
                                    c.split_ascii_whitespace().map(|class| (1, class))
                                }))
                                .chain(node.attrs.keys().map(|name| (3, name.as_str())))
                        {
                            if let Some(list) = buckets.get(&key) {
                                candidates.extend(list);
                            }
                        }
                        candidates.sort_unstable();
                        candidates.dedup();
                    }
                    let matched: Vec<_> = candidates
                        .iter()
                        .filter_map(|&order| {
                            element_selectors[order]
                                .iter()
                                .filter(|selector| selector.matches_reusing(doc, id, matches))
                                .map(|selector| selector.specificity)
                                .max()
                                .map(|specificity| (order, specificity))
                        })
                        .collect();
                    let matched_pseudos: Vec<_> = if node.tag.starts_with('#') {
                        Vec::new()
                    } else {
                        pseudo_rules
                            .iter()
                            .enumerate()
                            .filter_map(|(index, (_, selector, ..))| {
                                let candidate = match selector.key() {
                                    Some((0, value)) => {
                                        node.attrs.get("id").is_some_and(|id| id == value)
                                    }
                                    Some((1, value)) => {
                                        node.attrs.get("class").is_some_and(|classes| {
                                            classes
                                                .split_ascii_whitespace()
                                                .any(|class| class == value)
                                        })
                                    }
                                    Some((2, value)) => node.tag == value,
                                    Some((3, name)) => node.attrs.contains_key(name),
                                    _ => true,
                                };
                                (candidate && selector.matches_reusing(doc, id, matches))
                                    .then_some(index)
                            })
                            .collect()
                    };
                    matches.dependencies.sort_unstable();
                    matches.dependencies.dedup();
                    scoped.nodes[id] = Some(MatchedRules {
                        checked: std::cell::Cell::new(matches.epoch),
                        cached: None,
                        dependencies: std::mem::take(&mut matches.dependencies),
                        rules: matched.clone(),
                        pseudos: matched_pseudos.clone(),
                    });
                    (matched, matched_pseudos)
                };
                let resolved = StyleKey {
                    scope,
                    tag: node.tag.clone(),
                    classes: tailwind.then(|| node.attrs.get("class").cloned()).flatten(),
                    inline: node
                        .inline
                        .as_ref()
                        .map(Declarations::source)
                        .or_else(|| node.attrs.get("style").cloned()),
                    parent: parent_style.cloned().map(StyleRef),
                    font: font_key,
                    root: root_font,
                    rules: matched,
                    pseudos: matched_pseudos,
                    host: host.cloned(),
                    selection: selection.cloned(),
                };
                let value = style_cache.values.get(&resolved).cloned();
                key = Some(resolved);
                value
            });
        if let Some(cached) = cached {
            if let Some(key) = &key {
                scoped.nodes[id].as_mut().unwrap().cached =
                    Some(CachedNode::new(key, cached.clone()));
            }
            styles[id] = cached.style.clone();
            lengths[id] = cached.lengths;
            if id == 0 || node.tag == "html" {
                root = cached.lengths.font;
            }
            if let Some(values) = &cached.pseudos {
                pseudos.insert(id, values.clone());
            }
            if let Some(values) = &cached.selection {
                selections.insert(id, values.clone());
            }
            if let Some(values) = &cached.animations {
                animations.insert(id, values.clone());
            }
            if node.tag == "apps-shadow-root" {
                host_declarations = cached.host.clone();
            }
            continue;
        }
        let key = key.unwrap();
        let mut style = Style::new();
        if id == 0 && tailwind {
            style = theme.clone();
        }
        if let Some(parent) = parent_style {
            let leaf = matches!(node.tag.as_str(), "#text" | "#comment");
            for (key, value) in parent.iter() {
                if key.starts_with("--") && !leaf
                    || matches!(
                        key.as_str(),
                        "color"
                            | "font-size"
                            | "font-family"
                            | "font-weight"
                            | "line-height"
                            | "text-align"
                            | "white-space"
                            | "color-scheme"
                            | "fill"
                            | "pointer-events"
                            | "caret-color"
                            | "cursor"
                            | "visibility"
                    )
                {
                    style.insert(key.clone(), value.clone());
                }
            }
        }
        style.entry("font-size".into()).or_insert("16px".into());
        style.entry("color".into()).or_insert("#171717".into());
        if matches!(node.tag.as_str(), "button" | "meter" | "progress" | "select") {
            style.insert("user-select".into(), "none".into());
        }
        if node.tag == "slot" {
            style.insert("display".into(), "contents".into());
        }
        if !tailwind && node.tag == "body" {
            style.insert("margin".into(), "8px".into());
        }
        if matches!(
            node.tag.as_str(),
            "head" | "script" | "style" | "title" | "meta" | "link"
        ) {
            style.insert("display".into(), "none".into());
        }
        if !tailwind && node.tag == "h1" {
            style.insert("font-size".into(), "32px".into());
            style.insert("margin".into(), "20px 0".into());
        }
        if !tailwind && node.tag == "h2" {
            style.insert("font-size".into(), "24px".into());
            style.insert("margin".into(), "16px 0".into());
        }
        if !tailwind && node.tag == "p" {
            style.insert("margin".into(), "12px 0".into());
        }
        if tailwind && let Some(classes) = node.attrs.get("class") {
            let mut matched: Vec<_> = classes
                .split_ascii_whitespace()
                .filter_map(|class| {
                    utilities[class]
                        .as_ref()
                        .map(|(breakpoint, values)| (class, *breakpoint, values))
                })
                .collect();
            matched.sort_unstable_by_key(|(class, breakpoint, values)| {
                (*breakpoint, Reverse(values.len()), *class)
            });
            for (_, _, values) in matched {
                for (key, value) in values {
                    style.insert(key.clone(), value.clone());
                }
            }
        }
        let mut cascade: BTreeMap<String, (CascadeRank, String)> = BTreeMap::new();
        for &(order, [ids, classes, tags]) in &key.rules {
            let (_, declarations, layer) = &rules[order];
            for (key, value) in declarations {
                let important = value.trim_end().ends_with("!important");
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    order,
                );
                if cascade.get(key).is_none_or(|(old, _)| rank >= *old) {
                    cascade.insert(
                        key.clone(),
                        (rank, value.trim_end_matches("!important").trim().into()),
                    );
                }
            }
        }
        let inline = node.inline.as_ref().map(Cow::Borrowed).or_else(|| {
            node.attrs
                .get("style")
                .map(|source| Cow::Owned(Declarations::parse(source)))
        });
        if let Some(inline) = inline {
            for (key, value) in inline.resolve(tailwind.then_some(&theme), width) {
                let rank = (
                    value.trim_end().ends_with("!important"),
                    i32::MAX,
                    [1, 0, 0, 0],
                    rules.len(),
                );
                if cascade.get(&key).is_none_or(|(old, _)| rank >= *old) {
                    cascade.insert(
                        key,
                        (rank, value.trim_end_matches("!important").trim().into()),
                    );
                }
            }
        }
        if let Some(declarations) = context.and_then(|context| context.hosts.get(&id)) {
            for (key, value) in declarations {
                let important = value.ends_with("!important");
                if important || !cascade.contains_key(key) {
                    cascade.insert(
                        key.clone(),
                        (
                            (important, 0, [0; 4], 0),
                            value.trim_end_matches("!important").trim().into(),
                        ),
                    );
                }
            }
        }
        if node.tag == "apps-shadow-root" {
            for (key, (rank, value)) in &cascade {
                host_declarations.insert(
                    key.clone(),
                    if rank.0 {
                        format!("{value}!important")
                    } else {
                        value.clone()
                    },
                );
            }
        }
        for (key, (_, value)) in cascade {
            style.insert(key, value);
        }
        resolve_style(&mut style, parent_style.map(Rc::as_ref));
        let mut units = Lengths {
            font: parent,
            root,
            viewport: [width, height],
            container: None,
        };
        let size = units
            .resolve(&style["font-size"], parent.em)
            .filter(|size| *size >= 0.)
            .unwrap_or(parent.em);
        units.font = FontUnits::new(size);
        if let Some(text) = text_system {
            let font = font(&style);
            let metrics = font_metrics.entry(font.clone()).or_insert_with(|| {
                let id = text.resolve_font(&font);
                let size = gpui::px(1.);
                [
                    text.ch_advance(id, size).map(f32::from).unwrap_or(0.5),
                    f32::from(text.x_height(id, size)),
                    f32::from(text.cap_height(id, size)),
                    text.advance(id, size, '水')
                        .map(|s| f32::from(s.width))
                        .unwrap_or(1.),
                    f32::from(text.ascent(id, size) - text.descent(id, size)),
                ]
            });
            let [ch, ex, cap, ic, lh] = metrics.map(|value| value * size);
            units.font = FontUnits {
                em: size,
                ch,
                ex,
                cap,
                ic,
                lh,
            };
        }
        if let Some(value) = style.get("line-height") {
            let mut line_units = units;
            line_units.font.lh = parent.lh;
            if node.tag == "html" {
                line_units.root = units.font;
                line_units.root.lh = parent.lh;
            }
            if let Some((value, dimensional)) = line_units.parse(value, size)
                && value >= 0.
            {
                units.font.lh = if dimensional { value } else { value * size };
                style.insert(
                    "line-height".into(),
                    if dimensional {
                        format!("{value}px")
                    } else {
                        value.to_string()
                    },
                );
            }
        }
        if id == 0 || node.tag == "html" {
            root = units.font;
        }
        units.root = root;
        style.insert("font-size".into(), format!("{size}px"));
        let mut pseudo_cascade: [BTreeMap<String, (CascadeRank, String)>; 3] = Default::default();
        for &index in &key.pseudos {
            let (pseudo, selector, declarations, layer, order) = &pseudo_rules[index];
            let [ids, classes, tags] = selector.specificity;
            for (key, value) in *declarations {
                let important = value.trim_end().ends_with("!important");
                let rank = (
                    important,
                    if important { -*layer } else { *layer },
                    [0, ids, classes, tags],
                    *order,
                );
                if pseudo_cascade[*pseudo]
                    .get(key)
                    .is_none_or(|(old, _)| rank >= *old)
                {
                    pseudo_cascade[*pseudo].insert(
                        key.clone(),
                        (rank, value.trim_end_matches("!important").trim().into()),
                    );
                }
            }
        }
        let [before, after, selected] = pseudo_cascade;
        if !selected.is_empty()
            || node
                .parent
                .is_some_and(|parent| selections.contains_key(&parent))
        {
            let parent = node.parent.and_then(|parent| selections.get(&parent));
            let mut values = parent.cloned().unwrap_or_default();
            values.extend(style.iter().filter(|(key, _)| key.starts_with("--") || key.as_str() == "color-scheme").map(|(key, value)| (key.clone(), value.clone())));
            for (key, (_, value)) in selected {
                if matches!(key.as_str(), "color" | "background-color" | "-webkit-text-fill-color") {
                    let value = if matches!(value.as_str(), "inherit" | "unset") {
                        parent.and_then(|parent| parent.get(&key)).cloned().unwrap_or_else(|| {
                            if key == "background-color" { "transparent" } else { "currentColor" }.into()
                        })
                    } else { value };
                    values.insert(key, value);
                }
            }
            resolve_style(&mut values, parent);
            values.retain(|key, value| {
                if !matches!(key.as_str(), "color" | "background-color" | "-webkit-text-fill-color") {
                    return false;
                }
                if matches!(value.as_str(), "inherit" | "unset") {
                    *value = parent.and_then(|parent| parent.get(key)).cloned().unwrap_or_else(|| {
                        if key == "background-color" { "transparent" } else { "currentColor" }.into()
                    });
                } else if value == "initial" {
                    *value = if key == "background-color" { "transparent" } else { "currentColor" }.into();
                }
                value.eq_ignore_ascii_case("currentcolor") || color(value).is_some()
            });
            if !values.is_empty() {
                selections.insert(id, values);
            }
        }
        let pseudo_cascade = [before, after];
        if pseudo_cascade
            .iter()
            .any(|values| values.contains_key("content"))
        {
            let values = pseudo_cascade.map(|cascade| {
                let mut values = Style::new();
                for (key, value) in &style {
                    if key.starts_with("--")
                        || matches!(
                            key.as_str(),
                            "color" | "color-scheme" | "font-family" | "font-size" | "line-height"
                        )
                    {
                        values.insert(key.clone(), value.clone());
                    }
                }
                for (key, (_, value)) in cascade {
                    values.insert(key, value);
                }
                resolve_style(&mut values, Some(&style));
                values
            });
            pseudos.insert(id, Rc::new(values));
        }
        if style.contains_key("animation") || style.contains_key("animation-name") {
            let effects = Animation::parse(&style, &scoped.keyframes);
            if !effects.is_empty() {
                animations.insert(id, effects);
            }
        }
        let style = Rc::new(style);
        let cached = Rc::new(CachedStyle {
            style: style.clone(),
            lengths: units,
            pseudos: pseudos.get(&id).cloned(),
            selection: selections.get(&id).cloned(),
            animations: animations.get(&id).cloned(),
            host: if node.tag == "apps-shadow-root" {
                host_declarations.clone()
            } else {
                Style::new()
            },
        });
        scoped.nodes[id].as_mut().unwrap().cached = Some(CachedNode::new(&key, cached.clone()));
        style_cache.values.insert(key, cached);
        styles[id] = style;
        lengths[id] = units;
    }
    ComputedStyles {
        values: styles,
        lengths,
        pseudos,
        selections,
        user_select: vec![(UserSelect::None, 0); doc.nodes.len()],
        animations,
        children: HashMap::new(),
        host_declarations,
    }
}

fn resolve_style(style: &mut Style, parent: Option<&Style>) {
    if let Some(parent) = parent {
        for (key, value) in &mut *style {
            if value == "inherit"
                || value == "unset"
                    && (key.starts_with("--")
                        || matches!(
                            key.as_str(),
                            "color" | "font-size" | "font-family" | "line-height"
                        ))
            {
                *value = parent.get(key).cloned().unwrap_or_default();
            }
        }
    }
    style.retain(|key, value| !(key.starts_with("--") && value == "initial"));
    let resolved: Vec<_> = style
        .iter()
        .filter(|(_, value)| value.contains("var("))
        .map(|(key, value)| {
            let mut value = value.clone();
            for _ in 0..64 {
                let Some(start) = value.rfind("var(") else {
                    break;
                };
                let mut depth = 1;
                let Some(end) = value[start + 4..].char_indices().find_map(|(index, c)| {
                    if c == '(' {
                        depth += 1;
                    }
                    if c == ')' {
                        depth -= 1;
                    }
                    (depth == 0).then_some(start + 4 + index)
                }) else {
                    break;
                };
                let (name, fallback) = value[start + 4..end]
                    .split_once(',')
                    .unwrap_or((&value[start + 4..end], ""));
                let resolved = style
                    .get(name.trim())
                    .map(String::as_str)
                    .unwrap_or(fallback.trim())
                    .to_owned();
                value.replace_range(start..end + 1, &resolved);
            }
            (key.clone(), value)
        })
        .collect();
    for (key, value) in resolved {
        style.insert(key, value);
    }
    let dark = style
        .get("color-scheme")
        .is_some_and(|v| v == "dark" || v == "only dark");
    for (key, value) in &mut *style {
        if key.starts_with("--") {
            continue;
        }
        while let Some(start) = value.rfind("light-dark(") {
            let mut depth = 1;
            let Some(end) = value[start + 11..].char_indices().find_map(|(i, c)| {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                }
                (depth == 0).then_some(start + 11 + i)
            }) else {
                break;
            };
            let parts = components(&value[start + 11..end], ',');
            if parts.len() != 2 {
                break;
            }
            let selected = parts[usize::from(dark)].trim().to_owned();
            value.replace_range(start..=end, &selected);
        }
        if value == "inherit" {
            *value = parent.and_then(|p| p.get(key)).cloned().unwrap_or_default();
        }
        if (value.starts_with("color-mix(") || value.starts_with("rgb(from "))
            && (key.ends_with("color") || matches!(key.as_str(), "background" | "fill" | "stroke"))
            && let Some((rgb, alpha)) = color(value)
        {
            *value = format!("#{rgb:06x}{alpha:02x}");
        }
    }
    for (key, value) in &mut *style {
        if key.starts_with("border-") && (key.ends_with("width") || key == "border-color") {
            let parts = components(value, ' ');
            if parts.len() > 1 {
                if key.ends_with("width") {
                    *value = parts[0].to_owned();
                } else if let Some(color) = parts.iter().find(|v| color(v).is_some()) {
                    *value = (*color).to_owned();
                }
            }
        }
        let side = match key.as_str() {
            "padding-top" | "margin-top" | "row-gap" | "border-top-left-radius" => Some(0),
            "padding-right" | "margin-right" | "column-gap" | "border-top-right-radius" => Some(1),
            "padding-bottom" | "margin-bottom" | "border-bottom-right-radius" => Some(2),
            "padding-left" | "margin-left" | "border-bottom-left-radius" => Some(3),
            _ => None,
        };
        if let Some(side) = side {
            let parts = components(value, ' ');
            if parts.len() > 1 {
                *value = parts
                    .get(side)
                    .or_else(|| parts.get(side % 2))
                    .unwrap_or(&parts[0])
                    .to_string();
            }
        }
    }
}

pub(crate) fn selection_colors(style: &Style, selection: Option<&Style>) -> (Option<u32>, u32) {
    let normal = style.get("color").and_then(|value| color(value)).unwrap_or((0, 255));
    let foreground = selection.and_then(|style| style.get("color")).and_then(|value| {
        if value.eq_ignore_ascii_case("currentcolor") { Some(normal) } else { color(value) }
    });
    let background = selection.and_then(|style| style.get("background-color")).and_then(|value| {
        if value.eq_ignore_ascii_case("currentcolor") { Some(foreground.unwrap_or(normal)) } else { color(value) }
    }).unwrap_or(if foreground.is_some() { (0, 0) } else { (0x4488ff, 85) });
    let fill = selection.and_then(|style| style.get("-webkit-text-fill-color")).and_then(|value| {
        if value.eq_ignore_ascii_case("currentcolor") { Some(foreground.unwrap_or(normal)) } else { color(value) }
    }).or(foreground);
    (fill.map(|(color, alpha)| color << 8 | alpha as u32), background.0 << 8 | background.1 as u32)
}

#[derive(Clone)]
pub(crate) struct InlineText {
    pub text: Rc<str>,
    pub runs: Vec<gpui::TextRun>,
    pub ranges: Vec<(usize, std::ops::Range<usize>)>,
    pub backgrounds: Vec<(usize, u32)>,
}

pub(crate) fn inline_text(
    doc: &Document,
    styles: &ComputedStyles,
    id: usize,
) -> Option<InlineText> {
    let parent = &styles.values[id];
    if doc.nodes[id].children.is_empty()
        || !parent
            .get("white-space")
            .is_some_and(|value| value == "pre")
        || parent
            .get("display")
            .is_some_and(|value| matches!(value.as_str(), "grid" | "flex" | "inline-flex"))
    {
        return None;
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut ranges = Vec::new();
    let mut backgrounds = Vec::new();
    let mut pending: Vec<_> = doc.nodes[id]
        .children
        .iter()
        .rev()
        .map(|&child| (child, None, None))
        .collect();
    while let Some((child, end, background)) = pending.pop() {
        if let Some(index) = end {
            let (_, range): &mut (usize, std::ops::Range<usize>) = &mut ranges[index];
            range.end = text.len();
            continue;
        }
        let node = &doc.nodes[child];
        let style = &styles.values[child];
        if !matches!(
            node.tag.as_str(),
            "#text" | "span" | "b" | "strong" | "em" | "i" | "a" | "code" | "small"
        ) || styles.pseudos.contains_key(&child)
            || styles.animations.contains_key(&child)
            || styles.lengths[child].font.em != styles.lengths[id].font.em
            || styles.lengths[child].font.lh != styles.lengths[id].font.lh
            || ["white-space", "visibility", "pointer-events"]
                .iter()
                .any(|key| style.get(*key) != parent.get(*key))
            || style
                .get("cursor")
                .is_some_and(|value| Some(value) != parent.get("cursor"))
            || style.iter().any(|(key, value)| {
                !value.is_empty()
                    && !key.starts_with("--")
                    && !matches!(
                        key.as_str(),
                        "color"
                            | "font-size"
                            | "font-family"
                            | "font-weight"
                            | "line-height"
                            | "white-space"
                            | "color-scheme"
                            | "caret-color"
                            | "visibility"
                            | "pointer-events"
                            | "cursor"
                            | "fill"
                            | "text-align"
                            | "background-color"
                            | "border-radius"
                            | "border-top-left-radius"
                            | "border-top-right-radius"
                            | "border-bottom-right-radius"
                            | "border-bottom-left-radius"
                            | "box-decoration-break"
                            | "user-select"
                            | "box-sizing"
                    )
            })
        {
            return None;
        }
        let index = ranges.len();
        ranges.push((child, text.len()..text.len()));
        let inherited_background = background;
        let background = style
            .get("background-color")
            .and_then(|value| colors::color(value))
            .filter(|(_, alpha)| *alpha != 0)
            .or(background);
        if background != inherited_background
            && let Some((color, alpha)) = background
        {
            backgrounds.push((index, color << 8 | alpha as u32));
        }
        pending.push((child, Some(index), background));
        pending.extend(
            node.children
                .iter()
                .rev()
                .map(|&child| (child, None, background)),
        );
        if !node.text.is_empty() {
            let (color, alpha) = style
                .get("color")
                .and_then(|value| color(value))
                .unwrap_or((0, 255));
            runs.push(gpui::TextRun {
                len: node.text.len(),
                font: font(style),
                color: gpui::rgba(color << 8 | alpha as u32).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            text.push_str(&node.text);
        }
    }
    if text.contains(['\t', '\r'])
        || text.trim_end_matches('\n').contains('\n')
        || text.ends_with("\n\n")
    {
        return None;
    }
    if text.ends_with('\n') {
        text.pop();
        if let Some(run) = runs.last_mut() {
            run.len -= 1;
            if run.len == 0 {
                runs.pop();
            }
        }
        for (_, range) in &mut ranges {
            range.start = range.start.min(text.len());
            range.end = range.end.min(text.len());
        }
    }
    if text.is_empty() {
        return None;
    }
    Some(InlineText {
        text: text.into(),
        runs,
        ranges,
        backgrounds,
    })
}

pub(crate) fn font(style: &Style) -> gpui::Font {
    let mut font = gpui::font(
        style
            .get("font-family")
            .map_or(".SystemUIFont", String::as_str)
            .split(',')
            .next()
            .unwrap()
            .trim()
            .trim_matches(['\'', '"'])
            .to_owned(),
    );
    let weight = match style.get("font-weight").map(String::as_str) {
        Some("bold") => 700.,
        Some(value) => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(400.),
        None => 400.,
    };
    font.weight = gpui::FontWeight(weight.clamp(1., 1000.));
    font
}

pub fn declarations(source: &str) -> Style {
    Declarations::parse(source).resolve(None, 0.)
}

impl Declarations {
    fn resolve(&self, theme: Option<&Style>, width: f32) -> Style {
        let mut result = Style::new();
        for declaration in &self.0 {
            match declaration {
                Declaration::Apply(classes) => {
                    if let Some(theme) = theme {
                        let mut applied: Vec<_> = classes
                            .iter()
                            .map(String::as_str)
                            .filter_map(|class| {
                                crate::tailwind::utility(class, width, theme)
                                    .map(|(breakpoint, values)| (class, breakpoint, values))
                            })
                            .collect();
                        applied.sort_unstable_by_key(|(class, breakpoint, values)| {
                            (*breakpoint, Reverse(values.len()), *class)
                        });
                        for (_, _, values) in applied {
                            for (key, value) in values {
                                if result
                                    .get(&key)
                                    .is_none_or(|v: &String| !v.ends_with("!important"))
                                {
                                    result.insert(key, value);
                                }
                            }
                        }
                    }
                }
                Declaration::Property(key, value) => {
                    let key = key.as_str();
                    let important = value.ends_with("!important");
                    let plain = value.trim_end_matches("!important").trim();
                    let mut expanded = Vec::new();
                    match key {
                        "padding-inline" | "padding-block" | "margin-inline" | "margin-block"
                        | "inset-inline" | "inset-block" => {
                            let (property, axis) = key.split_once('-').unwrap();
                            let sides = if axis == "inline" {
                                ["left", "right"]
                            } else {
                                ["top", "bottom"]
                            };
                            let parts = components(plain, ' ');
                            for (index, side) in sides.into_iter().enumerate() {
                                let name = if property == "inset" {
                                    side.into()
                                } else {
                                    format!("{property}-{side}")
                                };
                                expanded
                                    .push((name, parts.get(index).copied().unwrap_or(parts[0])));
                            }
                        }
                        "overflow" => {
                            let parts = components(plain, ' ');
                            expanded.push(("overflow-x".into(), parts[0]));
                            expanded.push((
                                "overflow-y".into(),
                                parts.get(1).copied().unwrap_or(parts[0]),
                            ));
                        }
                        "border" | "border-top" | "border-right" | "border-bottom"
                        | "border-left" | "border-block" | "border-inline" => {
                            let sides: &[&str] = match key {
                                "border-top" => &["top"],
                                "border-right" => &["right"],
                                "border-bottom" => &["bottom"],
                                "border-left" => &["left"],
                                "border-block" => &["top", "bottom"],
                                "border-inline" => &["left", "right"],
                                _ => &["top", "right", "bottom", "left"],
                            };
                            let parts = components(plain, ' ');
                            for &part in &parts {
                                if matches!(part, "solid" | "dashed" | "dotted" | "none") {
                                    expanded.push(("border-style".into(), part));
                                } else if color(part).is_some() || part.starts_with("var(") {
                                    expanded.push(("border-color".into(), part));
                                    if part.starts_with("var(") && parts.len() == 1 {
                                        for side in sides {
                                            expanded.push((format!("border-{side}-width"), part));
                                        }
                                    }
                                } else {
                                    for side in sides {
                                        expanded.push((format!("border-{side}-width"), part));
                                    }
                                }
                            }
                        }
                        "inset" => {
                            let parts = components(plain, ' ');
                            for (index, side) in
                                ["top", "right", "bottom", "left"].into_iter().enumerate()
                            {
                                expanded.push((
                                    side.into(),
                                    parts
                                        .get(index)
                                        .or(parts.get(index % 2))
                                        .copied()
                                        .unwrap_or(parts[0]),
                                ));
                            }
                        }
                        "padding" | "margin" => {
                            for side in ["top", "right", "bottom", "left"] {
                                expanded.push((format!("{key}-{side}"), plain));
                            }
                        }
                        "gap" => {
                            expanded.push(("row-gap".into(), plain));
                            expanded.push(("column-gap".into(), plain));
                        }
                        "border-radius" => {
                            for corner in ["top-left", "top-right", "bottom-right", "bottom-left"] {
                                expanded.push((format!("border-{corner}-radius"), plain));
                            }
                        }
                        "background" if color(plain).is_some() || plain.starts_with("var(") => {
                            expanded.push(("background-color".into(), plain));
                        }
                        _ => {}
                    }
                    expanded.push((key.to_owned(), plain));
                    for (key, value) in expanded {
                        if important || result.get(&key).is_none_or(|v| !v.ends_with("!important"))
                        {
                            result.insert(
                                key,
                                format!("{value}{}", if important { " !important" } else { "" }),
                            );
                        }
                    }
                }
            }
        }
        result
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

impl Stylesheet {
    pub(crate) fn matches_media(&self, width: f32, height: f32) -> bool {
        let reduced_motion = reduced_motion();
        let theme = crate::browser::theme_state();
        matches!(self.0.as_slice(), [Rule::Media(queries, _)] if queries.iter().any(|query| {
            query.iter().all(|(feature, value)| feature.matches(value, width, height, reduced_motion, theme))
        }))
    }

    pub(crate) fn bind_parents(&mut self) -> anyhow::Result<()> {
        for rule in &mut self.0 {
            rule.bind_parent(None)?;
        }
        Ok(())
    }

    pub(crate) fn source(&self) -> String {
        self.0.iter().map(Rule::source).collect()
    }
}

pub fn edges(style: &Style, name: &str, base: f32, font: f32, vw: f32, vh: f32) -> [f32; 4] {
    Lengths::new(font, vw, vh).edges(style, name, base)
}

impl Lengths {
    pub fn edges(&self, style: &Style, name: &str, base: f32) -> [f32; 4] {
        let values: Vec<_> = components(style.get(name).map_or("0", String::as_str), ' ')
            .into_iter()
            .map(|v| self.resolve(v, base).unwrap_or(0.))
            .collect();
        let first = values.first().copied().unwrap_or(0.);
        let mut result = [
            first,
            values.get(1).copied().unwrap_or(first),
            values.get(2).copied().unwrap_or(first),
            values.get(3).or(values.get(1)).copied().unwrap_or(first),
        ];
        for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
            if let Some(value) = style.get(&format!("{name}-{side}")) {
                result[i] = self.resolve(value, base).unwrap_or(0.);
            }
        }
        result
    }
}

pub fn length(value: &str, base: f32, font: f32, vw: f32, vh: f32) -> f32 {
    Lengths::new(font, vw, vh)
        .parse(value, base)
        .map(|(value, _)| value)
        .unwrap_or(0.)
}

pub(crate) fn components(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut input = cssparser::Parser::new(value);
    let mut start = input.position();
    loop {
        let position = input.position();
        let Ok(token) = input.next_including_whitespace() else {
            break;
        };
        if matches!(token, cssparser::Token::Comma) && separator == ','
            || matches!(token, cssparser::Token::WhiteSpace(_)) && separator == ' '
        {
            let part = input.slice(start..position).trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = input.position();
        } else if matches!(
            token,
            cssparser::Token::Function(_)
                | cssparser::Token::ParenthesisBlock
                | cssparser::Token::SquareBracketBlock
                | cssparser::Token::CurlyBracketBlock
        ) && input
            .parse_nested_block(|input| {
                input
                    .expect_no_error_token()
                    .map_err(cssparser::ParseError::<()>::from)
            })
            .is_err()
        {
            return Vec::new();
        }
    }
    let part = input.slice_from(start).trim();
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

impl Lengths {
    pub fn shadows(&self, value: &str) -> Vec<gpui::BoxShadow> {
        let mut shadows = Vec::new();
        if value == "none" {
            return shadows;
        }
        for shadow in components(value, ',') {
            let mut lengths = Vec::new();
            let mut ink = (0, 255);
            for part in components(shadow, ' ') {
                if let Some(color) = color(part) {
                    ink = color;
                } else if let Some(value) = self.resolve(part, self.viewport[0]) {
                    lengths.push(value);
                } else {
                    return Vec::new();
                }
            }
            if !(2..=4).contains(&lengths.len()) {
                return Vec::new();
            }
            shadows.push(gpui::BoxShadow {
                color: gpui::rgba((ink.0 << 8) | u32::from(ink.1)).into(),
                offset: gpui::point(gpui::px(lengths[0]), gpui::px(lengths[1])),
                blur_radius: gpui::px(lengths.get(2).copied().unwrap_or(0.).max(0.)),
                spread_radius: gpui::px(lengths.get(3).copied().unwrap_or(0.)),
            });
        }
        shadows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_property_initial_uses_the_var_fallback() {
        let doc = Document::parse(
            "<style>section{--ink:#f00;--background:#fff}.corner{background-color:var(--ink,var(--background))}#reset{--ink:initial}#inherit{--ink:unset}</style><section><div id=reset class=corner><span class=corner></span></div><div id=inherit class=corner></div></section>",
        );
        let styles = styles(&doc, 800., 600.);
        assert_eq!(styles[element(&doc, "#reset")]["background-color"], "#fff");
        assert_eq!(styles[element(&doc, "span")]["background-color"], "#fff");
        assert!(!styles[element(&doc, "#reset")].contains_key("--ink"));
        assert_eq!(
            styles[element(&doc, "#inherit")]["background-color"],
            "#f00"
        );
    }

    #[test]
    fn selector_dependencies_follow_tree_and_attribute_changes() {
        let mut doc = Document::parse(
            "<style>.on span{color:red}i+span{font-weight:700}span:nth-child(2){background-color:yellow}div:has(.mark) span{color:blue}span:empty::before{content:'empty'}</style><div><i>head</i><span id=a>alpha</span><span id=b></span></div>",
        );
        let parent = element(&doc, "div");
        let a = element(&doc, "#a");
        let b = element(&doc, "#b");
        let sibling = element(&doc, "i");
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        for step in 0..45 {
            match step {
                1 => {
                    doc.nodes[parent].attrs.insert("class".into(), "on".into());
                }
                2 => {
                    doc.nodes[b].attrs.insert("class".into(), "mark".into());
                }
                3 => {
                    doc.nodes[parent].children.swap(0, 1);
                }
                4 => {
                    doc.nodes[b].text = "content".into();
                }
                5 => {
                    doc.nodes[sibling].tag = "#text".into();
                }
                6 => {
                    doc.nodes[b].attrs.remove("class");
                    doc.nodes[a].attrs.insert("class".into(), "mark".into());
                }
                7 => {
                    doc.nodes[a].inline = Some(Declarations::parse("color:orange"));
                }
                8 => {
                    doc.nodes[parent].children.retain(|&id| id != a);
                    doc.nodes[b].children.push(a);
                    doc.nodes[a].parent = Some(b);
                }
                _ => {
                    doc.nodes[b].attrs.insert("title".into(), step.to_string());
                }
            }
            let cached = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
            let fresh = compute_styles(
                &doc,
                800.,
                600.,
                None,
                &mut sheets,
                &mut StyleCache::default(),
                None,
            );
            assert_eq!(cached.values, fresh.values, "mutation {step}");
            assert_eq!(cached.pseudos, fresh.pseudos, "mutation {step}");
        }
    }

    #[test]
    fn inline_runs_keep_node_ranges_and_reject_independent_boxes() {
        let mut doc = Document::parse(
            "<pre style='white-space:pre;cursor:text'><span style='color:red;cursor:text'>A😀</span><span style='background-color:#ffee00'><b>B</b>C</span>\n</pre>",
        );
        let id = element(&doc, "pre");
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let inline = inline_text(&doc, &computed, id).unwrap();
        assert_eq!(&*inline.text, "A😀BC");
        assert_eq!(inline.backgrounds.len(), 1);
        assert_eq!(inline.backgrounds[0].1, 0xffee00ff);
        assert_eq!(
            inline.runs.iter().map(|run| run.len).sum::<usize>(),
            inline.text.len()
        );
        for (id, range) in &inline.ranges {
            assert_eq!(
                &inline.text[range.clone()],
                doc.text(*id).trim_end_matches('\n')
            );
        }
        let span = element(&doc, "span");
        doc.nodes[span].inline = None;
        doc.nodes[span]
            .attrs
            .insert("style".into(), "padding:4px".into());
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert!(inline_text(&doc, &computed, id).is_none());
    }

    #[test]
    fn rounded_inline_highlights_keep_one_text_layout_when_tokens_split() {
        let mut expected = None;
        for content in [
            "<span>  state: </span><span style='background-color:#abcdef;border-radius:3px;box-decoration-break:clone'>ReviewState | 'all'</span>",
            "<span>  </span><span>state</span><span>:</span><span> </span><span style='background-color:#abcdef;border-radius:3px;box-decoration-break:clone'><span>ReviewState</span><span> </span><span>|</span><span> </span><span>'all'</span></span>",
        ] {
            let doc = Document::parse(&format!("<pre style='white-space:pre'>{content}</pre>"));
            let id = element(&doc, "pre");
            let computed = compute_styles(
                &doc, 800., 600., None,
                &mut HashMap::new(), &mut StyleCache::default(), None,
            );
            let inline = inline_text(&doc, &computed, id).unwrap();
            let actual = (inline.text.clone(), inline.backgrounds.iter().map(|&(index, color)| {
                (inline.ranges[index].1.clone(), color)
            }).collect::<Vec<_>>());
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                assert_eq!(&*actual.0, "  state: ReviewState | 'all'");
                assert_eq!(actual.1, vec![(9..28, 0xabcdefff)]);
                expected = Some(actual);
            }
        }
    }

    #[test]
    fn cached_styles_follow_selectors_inheritance_and_stylesheets() {
        let mut doc = Document::parse(
            "<style>div{--ink:red}span{color:var(--ink)}div:has(b) span{color:blue}span::selection{background-color:var(--ink)}</style><div><span>one</span><span>two</span></div>",
        );
        let mut sheets = HashMap::new();
        let mut cache = StyleCache::default();
        let parent = doc.nodes.iter().position(|node| node.tag == "div").unwrap();
        let children: Vec<_> = doc
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(id, node)| (node.tag == "span").then_some(id))
            .collect();
        let first = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        let second = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert!(Rc::ptr_eq(
            &first.values[children[0]],
            &second.values[children[0]]
        ));
        assert!(Rc::ptr_eq(
            &second.values[children[0]],
            &second.values[children[1]]
        ));
        doc.nodes[parent]
            .attrs
            .insert("style".into(), "--ink:green".into());
        let inherited = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(inherited.values[children[0]]["color"], "green");
        assert_eq!(
            inherited.selections[&children[0]]["background-color"],
            "green"
        );
        doc.nodes[children[1]].tag = "b".into();
        let matched = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(matched.values[children[0]]["color"], "blue");
        let sheet = doc
            .nodes
            .iter()
            .position(|node| node.tag == "style")
            .unwrap();
        doc.nodes[sheet].text = "span{color:purple}".into();
        doc.nodes[sheet].stylesheet = None;
        let changed = compute_styles(&doc, 800., 600., None, &mut sheets, &mut cache, None);
        assert_eq!(changed.values[children[0]]["color"], "purple");
    }

    #[test]
    fn selection_colors_follow_highlight_inheritance() {
        let doc = Document::parse(r#"<style>
            div { color: black; background: yellow; --ink: red }
            div::selection { color: white; background: blue; font-size: 80px }
            span { color: green; --ink: purple }
            #inherit::selection { color: inherit; background-color: unset }
            #current::selection { color: currentColor; background-color: currentColor }
            #variable::selection { color: var(--ink); background: transparent }
            #initial::selection { color: initial; background-color: initial }
            </style><div><span id=inherit>inherit</span><span id=current>current</span><span id=variable>variable</span><span id=initial>initial</span></div>"#);
        let computed = compute_styles(&doc, 800., 600., None,
            &mut HashMap::new(), &mut StyleCache::default(), None);
        for (selector, expected) in [
            ("#inherit", (Some(0xffffffff), 0x0000ffff)),
            ("#current", (Some(0x008000ff), 0x008000ff)),
            ("#variable", (Some(0x800080ff), 0x00000000)),
            ("#initial", (Some(0x008000ff), 0x00000000)),
        ] {
            let id = element(&doc, selector);
            assert_eq!(selection_colors(&computed.values[id], computed.selections.get(&id)), expected, "{selector}");
            assert!(!computed.selections[&id].contains_key("font-size"));
            for child in &doc.nodes[id].children {
                assert_eq!(selection_colors(&computed.values[*child], computed.selections.get(child)), expected, "{selector} text");
            }
        }
    }

    #[test]
    fn selection_colors_do_not_copy_element_backgrounds() {
        for (rule, expected) in [
            ("color: red", (Some(0xff0000ff), 0)),
            ("background: blue", (None, 0x0000ffff)),
            ("color: transparent; background: transparent", (Some(0), 0)),
            ("-webkit-text-fill-color: yellow; color: red", (Some(0xffff00ff), 0)),
            ("font-size: 80px; background-image: linear-gradient(red, blue)", (None, 0x4488ff55)),
        ] {
            let doc = Document::parse(&format!("<style>div{{color:green;background:yellow}}div::selection{{{rule}}}</style><div>text</div>"));
            let computed = compute_styles(&doc, 800., 600., None,
                &mut HashMap::new(), &mut StyleCache::default(), None);
            let id = element(&doc, "div");
            assert_eq!(selection_colors(&computed.values[id], computed.selections.get(&id)), expected, "{rule}");
        }
    }

    #[test]
    fn editor_media_caret_and_selection_styles() {
        let doc = Document::parse(
            "<style>div{caret-color:red}::selection{background-color:#0000}@media(width>=480px){div{caret-color:#0000}}@media(prefers-reduced-motion:no-preference){div{animation:1s blink}}</style><div><span>text</span></div>",
        );
        let id = doc
            .nodes
            .iter()
            .position(|node| node.text == "text")
            .unwrap();
        let narrow = compute_styles(
            &doc,
            479.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let wide = compute_styles(
            &doc,
            480.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(narrow.values[id]["caret-color"], "red");
        assert_eq!(wide.values[id]["caret-color"], "#0000");
        assert_eq!(wide.selections[&id]["background-color"], "#0000");
        let div = doc.nodes[id]
            .parent
            .and_then(|parent| doc.nodes[parent].parent)
            .unwrap();
        assert_eq!(
            wide.values[div].contains_key("animation"),
            !reduced_motion()
        );
        assert!(MediaFeature::ReducedMotion.matches("reduce", 800., 600., true, 0));
        assert!(!MediaFeature::ReducedMotion.matches("no-preference", 800., 600., true, 0));
    }

    #[test]
    fn layers_supports_and_generated_content_keep_the_cascade() {
        let doc = Document::parse(
            r#"<style>
            @layer base, theme;
            @layer theme { .item { color: blue; background-color: blue !important; } }
            @layer base { #item { color: red; background-color: red !important; } .item::before { content: ""; width: 4px; background-color: red; } }
            @supports (display:grid) { .item { display:grid; } }
            @supports (width:1cqi) { .item { width:100cqi; } }
            @media (pointer:fine) { .item { cursor:pointer; } }
        </style><div id="item" class="item"></div>"#,
        );
        let id = element(&doc, "#item");
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert_eq!(computed.values[id]["color"], "blue");
        assert_eq!(computed.values[id]["background-color"], "red");
        assert_eq!(computed.values[id]["display"], "grid");
        assert_eq!(computed.values[id]["width"], "100cqi");
        assert_eq!(computed.values[id]["cursor"], "pointer");
        assert!(!computed.values[id].contains_key("content"));
        assert_eq!(computed.pseudos[&id][0]["background-color"], "red");
        assert_eq!(computed.pseudos[&id][0]["width"], "4px");
    }

    #[test]
    fn logical_borders_and_theme_colors_resolve_variables() {
        let doc = Document::parse(
            r#"<style>
            :host { --edge:2px solid #fff; --bg:#fff; --ink:light-dark(#f00,#0f0); color-scheme:dark; }
            .item { border-right:var(--edge); border-left:1px solid var(--bg); padding-inline:1ch 2ch; margin-block:8px; color:var(--ink); background-color:color-mix(in lab, white 50%, black); }
            .item span { background-color:light-dark(inherit,inherit); }
        </style><apps-shadow-root><div class="item"><span>text</span></div></apps-shadow-root>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let item = element(&doc, ".item");
        assert_eq!(computed[item]["border-right-width"], "2px");
        assert_eq!(computed[item]["border-left-width"], "1px");
        assert_eq!(computed[item]["border-color"], "#fff");
        assert_eq!(computed[item]["padding-left"], "1ch");
        assert_eq!(computed[item]["padding-right"], "2ch");
        assert_eq!(computed[item]["margin-bottom"], "8px");
        assert_eq!(computed[item]["color"], "#0f0");
        assert_eq!(
            color(&computed[item]["background-color"]),
            Some((0x777777, 255))
        );
        assert_eq!(
            computed[element(&doc, "span")]["background-color"],
            computed[item]["background-color"]
        );
        assert_eq!(color("rgb(from #d52c36 r g b / .15)"), Some((0xd52c36, 38)));
    }

    #[test]
    fn declaration_tokens_preserve_strings_blocks_and_priority() {
        let style = declarations(
            r#"/* leading ; } */ COLOR: red ! /* priority */ IMPORTANT;
            color: blue; p\61 dding: 2px /* gap */ 4px;
            --Case: { key: [a;b]; nested: fn(";}"); };
            --case: "/* text */;escaped\"}";
            --tokens: one/**/two;
            content: "!important"; width: calc(10px /* math */ + 2px);"#,
        );
        assert_eq!(style["color"], "red !important");
        assert_eq!(style["padding"], "2px  4px");
        assert_eq!(style["--Case"], r#"{ key: [a;b]; nested: fn(";}"); }"#);
        assert_eq!(style["--case"], r#""/* text */;escaped\"}""#);
        assert_eq!(style["--tokens"], "one/**/two");
        assert_eq!(style["content"], r#""!important""#);
        assert_eq!(length(&style["width"], 0., 16., 800., 600.), 12.);
    }

    #[test]
    fn invalid_declarations_recover_at_the_next_property() {
        let style = declarations(
            "color: red; broken; width: ]; height: 7px; content: \"bad\n; \
             padding: 3px; background: url(bad\"url); margin: 4px; \
             @unknown { color: blue; } border-width: 2px; \
             color: green !important extra; opacity:; --empty:;",
        );
        assert_eq!(style["color"], "red");
        assert_eq!(style["height"], "7px");
        assert_eq!(style["padding"], "3px");
        assert_eq!(style["margin"], "4px");
        assert_eq!(style["border-width"], "2px");
        assert_eq!(style["--empty"], "");
        for key in ["width", "content", "background", "opacity"] {
            assert!(!style.contains_key(key), "{key}");
        }
    }

    #[test]
    fn tokenized_at_rules_keep_scope_and_accept_css_escapes() {
        let doc = Document::parse(
            r#"<style>
                @IMPORT url(tailwindcss);
                @THEME { --color-brand: #123456; }
                .box/**/.active {
                    @APPLY bg-brand p-[calc(1px_+_2px)];
                    COLOR: red !/**/IMPORTANT;
                    @MEDIA SCREEN/**/and (MIN-WIDTH: 500px) {
                        width: 10px;
                        > span { height: 20px; }
                    }
                    color: blue;
                }
                @unknown { .box { width: 999px; } }
                .box/**/span { height: 999px; }
                @m\65 dia (max-width: 499px) { .box { width: 5px; } }
            </style><div class="box active"><span></span></div>"#,
        );
        for (width, expected) in [(400., "5px"), (800., "10px")] {
            let computed = styles(&doc, width, 600.);
            let style = &computed[element(&doc, ".box")];
            assert_eq!(style["color"], "red");
            assert_eq!(style["width"], expected);
            assert_eq!(style["background-color"], "#123456");
            assert_eq!(edges(style, "padding", width, 16., width, 600.), [3.; 4]);
            assert_eq!(
                computed[element(&doc, "span")].contains_key("height"),
                width >= 500.
            );
        }
    }

    #[test]
    fn relative_units_follow_root_font_and_inherited_line_height() {
        let doc = Document::parse(
            r#"<html style="font-size:20px;line-height:1.5"><style>
            section { font-size:2em; line-height:150%; width:10rem; padding:calc(1lh / 2) 2ch; }
            #absolute { font-size:.5em; height:2lh; }
            #number { font-size:1rem; line-height:2; }
            #number span { font-size:.5em; height:1lh; }
            #relative { line-height:2lh; margin:1rlh; }
        </style><section><div id="absolute"></div><div id="number"><span></span></div>
        <div id="relative"></div></section></html>"#,
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let section = element(&doc, "section");
        let absolute = element(&doc, "#absolute");
        let number = element(&doc, "#number");
        let child = element(&doc, "span");
        let relative = element(&doc, "#relative");
        assert_eq!(computed.lengths[section].font.em, 40.);
        assert_eq!(computed.lengths[section].font.lh, 60.);
        assert_eq!(
            computed.lengths[section].resolve(&computed.values[section]["width"], 800.),
            Some(200.)
        );
        assert_eq!(
            computed.lengths[section].edges(&computed.values[section], "padding", 800.),
            [30., 40., 30., 40.]
        );
        assert_eq!(computed.lengths[absolute].font.em, 20.);
        assert_eq!(computed.lengths[absolute].font.lh, 60.);
        assert_eq!(computed.values[absolute]["line-height"], "60px");
        assert_eq!(computed.lengths[number].font.lh, 40.);
        assert_eq!(computed.lengths[child].font.em, 10.);
        assert_eq!(computed.lengths[child].font.lh, 20.);
        assert_eq!(computed.values[child]["line-height"], "2");
        assert_eq!(computed.lengths[relative].font.lh, 120.);
        assert_eq!(
            computed.lengths[relative].edges(&computed.values[relative], "margin", 800.),
            [30.; 4]
        );
    }

    #[test]
    fn root_units_resolve_without_circular_font_metrics() {
        let doc = Document::parse(
            "<html style='font-size:2rem;line-height:2rem'><div style='font-size:.5rem;line-height:calc(1 + .5);width:2rlh'></div></html>",
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let root = element(&doc, "html");
        let child = element(&doc, "div");
        assert_eq!(computed.lengths[root].font.em, 32.);
        assert_eq!(computed.lengths[root].font.lh, 64.);
        assert_eq!(computed.lengths[child].font.em, 16.);
        assert_eq!(computed.lengths[child].font.lh, 24.);
        assert_eq!(computed.lengths[child].resolve("2rlh", 0.), Some(128.));
        let doc = Document::parse(
            "<html style='line-height:2rlh'><div style='font-size:1lh'></div></html>",
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        assert!((computed.lengths[element(&doc, "div")].font.em - 38.4).abs() < 0.001);
    }

    #[test]
    fn math_values_work_in_variables_shorthands_shadows_and_utilities() {
        let doc = Document::parse(
            r#"<html style="font-size:20px"><style>
            @import 'tailwindcss';
            .box { --space:calc(1rem + 2px); padding:var(--space) min(4vw, 30px);
                box-shadow:0 1lh clamp(2px, 1vw, 10px) #000; }
        </style><div class="box h-[calc(100dvh_-_2rem)] w-[min(50vw,40ch)]"></div></html>"#,
        );
        let computed = compute_styles(
            &doc,
            800.,
            600.,
            None,
            &mut HashMap::new(),
            &mut StyleCache::default(),
            None,
        );
        let id = element(&doc, ".box");
        let style = &computed.values[id];
        let units = computed.lengths[id];
        assert_eq!(units.edges(style, "padding", 800.), [22., 30., 22., 30.]);
        assert_eq!(units.resolve(&style["height"], 600.), Some(560.));
        assert_eq!(units.resolve(&style["width"], 800.), Some(400.));
        let shadows = units.shadows(&style["box-shadow"]);
        assert_eq!(shadows[0].offset.y, gpui::px(24.));
        assert_eq!(shadows[0].blur_radius, gpui::px(8.));
    }

    #[test]
    fn media_queries_use_viewport_and_initial_font_units() {
        let doc = Document::parse(
            r#"<html style="font-size:32px"><style>
            @media (min-width:calc(40em + 1px)) and (min-height:300px) { .box { width:1px; } }
            @media print, (max-height:min(300px, 50vw)) { .box { height:2px; } }
            @media (max-width:invalid) { .box { padding:99px; } }
        </style><div class="box"></div></html>"#,
        );
        let id = element(&doc, ".box");
        for (width, height, matches_width, matches_height) in [
            (640., 600., false, false),
            (641., 600., true, false),
            (800., 200., false, true),
            (800., 600., true, false),
        ] {
            let computed = styles(&doc, width, height);
            assert_eq!(computed[id].contains_key("width"), matches_width);
            assert_eq!(computed[id].contains_key("height"), matches_height);
            assert!(!computed[id].contains_key("padding"));
        }
    }

    #[test]
    fn theme_colors_and_apply_reach_native_paint() {
        let doc = Document::parse(
            r#"<html><style>
            @import "tailwindcss";
            .select2-dropdown { @apply rounded-b-lg shadow-md; }
            @theme { --color-mint-500: oklch(0.72 0.11 178); }
        </style><div class="select2-dropdown bg-mint-500 text-mint-500 border-mint-500"></div></html>"#,
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        for property in ["background-color", "color", "border-color"] {
            assert_eq!(style[property], "oklch(0.72 0.11 178)");
            assert!(color(&style[property]).is_some());
        }
        assert_eq!(style["border-bottom-left-radius"], "8px");
        assert_eq!(style["border-bottom-right-radius"], "8px");
        assert!(!style.contains_key("border-top-left-radius"));
        let paint = Lengths::new(16., 800., 600.).shadows(&style["box-shadow"]);
        assert_eq!(paint.len(), 2);
        assert_eq!(paint[0].offset, gpui::point(gpui::px(0.), gpui::px(4.)));
        assert_eq!(paint[0].blur_radius, gpui::px(6.));
        assert_eq!(paint[0].spread_radius, gpui::px(-1.));
        assert!((paint[0].color.a - 0.1).abs() < 0.003);
    }

    #[test]
    fn theme_tokens_apply_order_and_scoped_overrides() {
        let doc = Document::parse(
            r#"<html><style>
            @import "tailwindcss";
            @theme {
                --color-mint-500: oklch(0.72 0.11 178);
                --color-accent: var(--color-mint-500);
                --spacing: 5px;
                --spacing-card: 23px;
                --radius-lg: 10px;
                --text-title: 2rem;
                --font-body: Helvetica;
                --font-weight-heavy: 850;
                --shadow-card: 0 2px 4px rgba(0, 0, 0, 0.25);
                --breakpoint-wide: 50rem;
            }
            .box {
                @apply rounded-b-lg bg-accent p-4;
                padding-left: 3px;
                @apply pl-2;
                color: blue !important;
                @apply text-mint-500;
                &.selected { --color-accent: #123456; }
            }
        </style><div class="box selected w-card -mt-card -mb-2 text-title font-body font-heavy shadow-card wide:w-8"></div></html>"#,
        );
        let style = styles(&doc, 799., 600.).pop().unwrap();
        assert_eq!(style["background-color"], "#123456");
        assert_eq!(style["color"], "blue");
        assert_eq!(style["border-bottom-left-radius"], "10px");
        assert_eq!(style["font-size"], "32px");
        assert_eq!(style["font-family"], "Helvetica");
        assert_eq!(style["font-weight"], "850");
        assert_eq!(
            edges(&style, "padding", 799., 32., 799., 600.),
            [20., 20., 20., 10.]
        );
        assert_eq!(
            edges(&style, "margin", 799., 32., 799., 600.),
            [-23., 0., -10., 0.]
        );
        assert_eq!(length(&style["width"], 799., 32., 799., 600.), 23.);
        assert_eq!(
            Lengths::new(32., 799., 600.)
                .shadows(&style["box-shadow"])
                .len(),
            1
        );
        let wide = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(length(&wide["width"], 800., 32., 800., 600.), 40.);
    }

    #[test]
    fn oklch_and_rgb_colors_have_expected_channels_and_alpha() {
        assert_eq!(color("oklch(0 0 0)"), Some((0, 255)));
        assert_eq!(color("oklch(100% 0 0 / 50%)"), Some((0xffffff, 128)));
        assert_eq!(
            color("oklch(0.62795536 0.25768331 29.233885)"),
            Some((0xff0000, 255))
        );
        assert_eq!(color("rgb(100% 0% 0% / 25%)"), Some((0xff0000, 64)));
        assert_eq!(color("rgba(0, 128, 255, 0.5)"), Some((0x0080ff, 128)));
        assert_eq!(color("oklch(NaN 0 0)"), None);
        assert_eq!(color("oklch(0.5 0)"), None);
        let doc = Document::parse(
            "<div style='color: var(--missing, oklch(0 0 0)); background: var(--missing, var(--fallback, #123456))'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(style["color"], "oklch(0 0 0)");
        assert_eq!(style["background-color"], "#123456");
    }

    fn element(doc: &Document, selector: &str) -> usize {
        (0..doc.nodes.len())
            .find(|&id| doc.matches(id, selector))
            .unwrap()
    }

    #[test]
    fn nested_selectors_and_combinators() {
        let doc = Document::parse(
            r#"<style>
                .card, .panel {
                    padding: 10px;
                    &.active { border-radius: 8px; }
                    h2, p {
                        margin: 0;
                        span { font-weight: 700; }
                    }
                    > p { background: red; }
                    & > h2 + p { width: 100px; }
                    h2 ~ p { height: 20px; }
                }
            </style><main>
                <section class="card active"><h2>Title</h2>text<p id="direct"><span>Text</span></p>
                    <div><p id="deep">Deep</p></div><p id="sibling">Last</p>
                </section>
                <section class="panel"><p id="panel">Panel</p></section>
                <p id="outside">Outside</p>
            </main>"#,
        );
        let computed = styles(&doc, 800., 600.);
        for (selector, key, value) in [
            (".card", "padding-left", "10px"),
            (".card", "border-radius", "8px"),
            ("#direct", "margin-top", "0"),
            ("#direct", "background-color", "red"),
            ("#direct", "width", "100px"),
            ("#direct", "height", "20px"),
            ("#sibling", "height", "20px"),
            ("#deep", "margin-top", "0"),
            ("#panel", "background-color", "red"),
            ("span", "font-weight", "700"),
        ] {
            let id = element(&doc, selector);
            assert_eq!(computed[id][key], value, "{selector} {key}");
        }
        for (selector, key) in [
            ("#deep", "background-color"),
            ("#deep", "height"),
            ("#sibling", "width"),
            ("#outside", "margin-top"),
            ("#outside", "background-color"),
        ] {
            let id = element(&doc, selector);
            assert!(!computed[id].contains_key(key), "{selector} {key}");
        }
    }

    #[test]
    fn nesting_keeps_parent_selector_context() {
        let doc = Document::parse(
            r#"<style>
                .outer .inner {
                    .context & { padding: 7px; }
                    div& { width: 10px; }
                    && { height: 20px; }
                }
                .a, .b {
                    & + & { margin: 3px; }
                    + & { border-radius: 4px; }
                }
            </style><main class="outer">
                <section class="context"><div class="inner" id="inside"></div></section>
                <div class="inner" id="outside"></div>
                <section class="inner" id="section"></section>
                <div class="a"></div>text<div class="b" id="mixed"></div>
            </main>"#,
        );
        let computed = styles(&doc, 800., 600.);
        for (selector, key, value) in [
            ("#inside", "padding-left", "7px"),
            ("#inside", "width", "10px"),
            ("#inside", "height", "20px"),
            ("#outside", "width", "10px"),
            ("#mixed", "margin-left", "3px"),
            ("#mixed", "border-radius", "4px"),
        ] {
            let id = element(&doc, selector);
            assert_eq!(computed[id][key], value, "{selector} {key}");
        }
        for (selector, key) in [("#outside", "padding"), ("#section", "width")] {
            let id = element(&doc, selector);
            assert!(!computed[id].contains_key(key), "{selector} {key}");
        }
    }

    #[test]
    fn nested_cascade_uses_parent_list_specificity_and_source_order() {
        let doc = Document::parse(
            r#"<style>
                .card {
                    color: red;
                    & { color: green; padding: 1px !important; }
                    color: blue;
                    padding: 2px;
                    & { padding: 3px !important; }
                    margin: 4px;
                }
                .card, #unused {
                    & { width: 10px; }
                    width: 20px;
                    & { max-width: 11px; }
                    max-width: 21px;
                    .child, span { height: 30px; }
                    && { border-radius: 5px; }
                }
                #card { width: 40px; border-radius: 6px; }
                .card .child { height: 50px; }
            </style><div id="card" class="card" style="width:60px;padding:9px">
                <span class="child"></span>
            </div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let card = element(&doc, "#card");
        assert_eq!(computed[card]["color"], "blue");
        assert_eq!(computed[card]["padding-left"], "3px");
        assert_eq!(computed[card]["margin-left"], "4px");
        assert_eq!(computed[card]["width"], "60px");
        assert_eq!(computed[card]["max-width"], "11px");
        assert_eq!(computed[card]["border-radius"], "5px");
        let child = element(&doc, ".child");
        assert_eq!(computed[child]["height"], "30px");
    }

    #[test]
    fn deep_nesting_shares_parent_selectors() {
        let source = format!(
            "<html><style>html {{ color: blue; }} & {{ color: red; }} .card {{{}&.active {{ width: 1px; }}{}}}</style><div class='card active'></div></html>",
            "&& {".repeat(80),
            "}".repeat(80),
        );
        let doc = Document::parse(&source);
        let computed = styles(&doc, 800., 600.);
        assert_eq!(computed[element(&doc, "html")]["color"], "blue");
        assert_eq!(computed[element(&doc, ".card")]["width"], "1px");
    }

    #[test]
    fn nested_media_preserves_scope_and_order() {
        let doc = Document::parse(
            r#"<style>
                @media (min-width: 300px) {
                    .card {
                        width: 10px;
                        @media (max-width: 600px) {
                            width: 20px;
                            > .child { height: 30px; }
                            @media (min-width: 400px) { padding: 5px; }
                        }
                        height: 40px;
                        @supports (display: unsupported) { width: 99px; & { height: 99px; } }
                        margin: 6px;
                    }
                }
            </style><div class="card"><div class="child"></div></div><div id="outside"></div>"#,
        );
        let card = element(&doc, ".card");
        let child = element(&doc, ".child");
        let outside = element(&doc, "#outside");
        for (width, expected, padding, child_height) in [
            (250., None, None, None),
            (350., Some("20px"), None, Some("30px")),
            (500., Some("20px"), Some("5px"), Some("30px")),
            (800., Some("10px"), None, None),
        ] {
            let computed = styles(&doc, width, 600.);
            assert_eq!(computed[card].get("width").map(String::as_str), expected);
            assert_eq!(
                computed[card].get("padding-left").map(String::as_str),
                padding
            );
            assert_eq!(
                computed[child].get("height").map(String::as_str),
                child_height
            );
            assert!(!computed[outside].contains_key("width"));
            if expected.is_some() {
                assert_eq!(computed[card]["height"], "40px");
                assert_eq!(computed[card]["margin-left"], "6px");
            }
        }
    }

    #[test]
    fn nested_parser_handles_values_comments_and_invalid_rules() {
        let doc = Document::parse(
            r#"<style>
                .card {
                    --quoted: "escaped\"}; /* text */";
                    --block: { one: [a;b]; two: { three: four; }; };
                    background-image: url(data:image/svg+xml;a={b});
                    /* } ; { */
                    &/* comment */.active { padding: 4px; }
                    &:unsupported { width: 99px; & { height: 99px; } }
                    &suffix { margin: 99px; }
                    > { color: red; }
                    color: blue;
                    .child { height: 10px }
                    width: 20px
                }
                .after { width: 30px; }
            </style><div class="card active"><div class="child"></div></div><div class="after"></div>"#,
        );
        let computed = styles(&doc, 800., 600.);
        let card = element(&doc, ".card");
        assert_eq!(computed[card]["--quoted"], r#""escaped\"}; /* text */""#);
        assert_eq!(
            computed[card]["--block"],
            "{ one: [a;b]; two: { three: four; }; }"
        );
        assert_eq!(
            computed[card]["background-image"],
            "url(data:image/svg+xml;a={b})"
        );
        assert_eq!(computed[card]["padding-left"], "4px");
        assert_eq!(computed[card]["color"], "blue");
        assert_eq!(computed[card]["width"], "20px");
        assert!(!computed[card].contains_key("height"));
        assert!(!computed[card].contains_key("margin"));
        let child = element(&doc, ".child");
        assert_eq!(computed[child]["height"], "10px");
        let after = element(&doc, ".after");
        assert_eq!(computed[after]["width"], "30px");
    }

    #[test]
    fn tailwind_import_is_opt_in_and_preserves_following_rules() {
        for import in [
            "",
            "/* @import \"tailwindcss\"; */",
            "p { content: '@import \"tailwindcss\";' }",
        ] {
            let doc = Document::parse(&format!(
                "<style>{import} .box {{ color: blue }}</style><div class='box h-4 w-4 flex'></div>"
            ));
            let style = styles(&doc, 800., 600.).pop().unwrap();
            assert!(!style.contains_key("height"));
            assert!(!style.contains_key("display"));
            assert_eq!(style["color"], "blue");
        }
        for import in [
            "@import \"tailwindcss\";",
            "@import 'tailwindcss';",
            "@import url('tailwindcss');",
        ] {
            let doc = Document::parse(&format!(
                "<style>{import} .box {{ color: blue }}</style><div class='box h-4 w-4 flex'></div>"
            ));
            let style = styles(&doc, 800., 600.).pop().unwrap();
            assert_eq!(length(&style["height"], 0., 16., 800., 600.), 16.);
            assert_eq!(length(&style["width"], 0., 16., 800., 600.), 16.);
            assert_eq!(style["display"], "flex");
            assert_eq!(style["color"], "blue");
        }
    }

    #[test]
    fn tailwind_classes_have_stable_order_and_respond_to_width() {
        for classes in [
            "p-8 px-4 pl-2 gap-4 gap-x-2 size-8 w-4 sm:w-8 md:w-12 lg:w-16",
            "lg:w-16 md:w-12 sm:w-8 w-4 size-8 gap-x-2 gap-4 pl-2 px-4 p-8",
        ] {
            let doc = Document::parse(&format!(
                "<style>@import 'tailwindcss';</style><div class='{classes}'></div>"
            ));
            for (width, expected) in [
                (639., "1rem"),
                (640., "2rem"),
                (768., "3rem"),
                (1024., "4rem"),
            ] {
                let style = styles(&doc, width, 600.).pop().unwrap();
                assert_eq!(style["width"], expected);
                assert_eq!(style["height"], "2rem");
                assert_eq!(
                    edges(&style, "padding", width, 16., width, 600.),
                    [32., 16., 32., 8.]
                );
                assert_eq!(style["row-gap"], "1rem");
                assert_eq!(style["column-gap"], "0.5rem");
            }
        }
    }

    #[test]
    fn css_and_inline_shorthands_override_utilities() {
        let doc = Document::parse(
            "<style>@import 'tailwindcss'; div { padding: 2px 3px; gap: 4px; background: blue; width: 20px !important; } .box { padding-left: 5px; }</style><div class='box p-8 bg-white gap-8 w-4' style='padding: 6px; gap: 7px 8px; background: black; width: 30px'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(edges(&style, "padding", 800., 16., 800., 600.), [6.; 4]);
        assert_eq!(style["background-color"], "black");
        assert_eq!(style["row-gap"], "7px");
        assert_eq!(style["column-gap"], "8px");
        assert_eq!(style["width"], "20px");
        let style = declarations(
            "padding: 1px !important; padding: 2px; padding-left: 3px; padding-right: 4px !important",
        );
        assert_eq!(style["padding-left"], "1px !important");
        assert_eq!(style["padding-right"], "4px !important");
        let doc = Document::parse(
            "<html><style>@import 'tailwindcss'; :root { --space: 2px 4px; } div { padding: var(--space); gap: var(--missing, 5px 6px); margin: var(--space); margin-left: 9px; }</style><div class='p-8 gap-8 m-8'></div></html>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(
            edges(&style, "padding", 800., 16., 800., 600.),
            [2., 4., 2., 4.]
        );
        assert_eq!(
            edges(&style, "margin", 800., 16., 800., 600.),
            [2., 4., 2., 9.]
        );
        assert_eq!(style["row-gap"], "5px");
        assert_eq!(style["column-gap"], "6px");
    }

    #[test]
    fn tailwind_sizes_colors_and_unknown_classes() {
        let doc = Document::parse(
            "<style>@import 'tailwindcss';</style><div class='grid grid-cols-2 min-h-screen max-w-full w-1/2 h-[30px] -mt-2 mx-auto border-2 border-[#123456] rounded-xl bg-[#abcdef] text-lg font-semibold hover:hidden w-NaN w-1/0 p--4 custom'></div>",
        );
        let style = styles(&doc, 800., 600.).pop().unwrap();
        assert_eq!(style["display"], "grid");
        assert_eq!(style["grid-template-columns"], "repeat(2,1fr)");
        assert_eq!(style["min-height"], "100vh");
        assert_eq!(style["max-width"], "100%");
        assert_eq!(style["width"], "50%");
        assert_eq!(style["height"], "30px");
        assert_eq!(style["margin-top"], "-0.5rem");
        assert_eq!(style["margin-left"], "auto");
        assert_eq!(style["margin-right"], "auto");
        assert_eq!(style["border-width"], "2px");
        assert_eq!(style["border-color"], "#123456");
        assert_eq!(style["border-bottom-left-radius"], "12px");
        assert_eq!(style["background-color"], "#abcdef");
        assert_eq!(style["font-size"], "18px");
        assert_eq!(style["font-weight"], "600");
        assert!(!style.contains_key("padding-top"));
    }

    #[test]
    fn cascade_variables_media_and_specificity() {
        let doc = Document::parse(
            "<html><head><style>:root{--ink:#123456}.box{color:red} #one{color:var(--ink)!important} .box{display:grid;grid-template-columns:repeat(2,1fr)} @media(max-width:500px){.box{grid-template-columns:1fr}}</style></head><body><div class='box' id='one' style='color:blue'>A</div></body></html>",
        );
        let id = doc
            .nodes
            .iter()
            .position(|n| n.attrs.get("id").is_some_and(|s| s == "one"))
            .unwrap();
        let wide = styles(&doc, 800., 600.);
        assert_eq!(wide[id]["color"], "#123456");
        assert_eq!(wide[id]["grid-template-columns"], "repeat(2,1fr)");
        let narrow = styles(&doc, 400., 600.);
        assert_eq!(narrow[id]["grid-template-columns"], "1fr");
    }
}
