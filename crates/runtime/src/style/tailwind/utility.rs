use super::*;
use crate::style::tailwind::tables::{
    BACKDROP, COLORS, DEFAULTS, EDGES, FILTER, MASK, NEGATIVE, ORIGINS, SHADOW, STATIC, STOPS,
    TRANSFORM, TRANSITION,
};
use crate::style::tailwind::value::{
    MATH, call, decode, direction, integer, is_color, is_length, layer, modifier, named_value,
    shadow_color, split, statements, valid,
};

impl Config {
    pub(super) fn compile(&self, class: &str, base: &str) -> Option<Compiled> {
        let mut parts = split(class, ':');
        if parts.len() > 16 {
            return None;
        }
        let utility = parts.pop()?;
        let (utility, important) = match utility
            .strip_suffix('!')
            .or_else(|| utility.strip_prefix('!'))
        {
            Some(utility) => (utility, true),
            None => (utility, false),
        };
        let body = self.utility(utility)?;
        let mut branches = vec![(Vec::<String>::new(), vec![base.to_owned()])];
        let mut ranks = Vec::new();
        let mut content = false;
        // Cap selector size: comma parts and & expansion multiply at each variant.
        let (mut size, limit) = (0, (base.len() + 64) * 256);
        for name in parts {
            let (rank, variant) = self.variant(name, 0)?;
            if variant.relative {
                return None;
            }
            ranks.push(rank);
            content |= variant.content;
            let mut next = Vec::new();
            for (at, selectors) in &branches {
                for (rules, template) in &variant.branches {
                    let mut at = at.clone();
                    at.extend(rules.iter().cloned());
                    let selectors = match template {
                        None => selectors.clone(),
                        Some(template) => {
                            let parts = split(template, ',');
                            let mut list = Vec::with_capacity(selectors.len() * parts.len());
                            for selector in selectors {
                                for part in &parts {
                                    let part = part.trim();
                                    size += part.len() + part.matches('&').count() * selector.len();
                                    if size > limit {
                                        return None;
                                    }
                                    list.push(part.replace('&', selector));
                                }
                            }
                            list
                        }
                    };
                    next.push((at, selectors));
                }
            }
            if next.len() > 16 {
                return None;
            }
            branches = next;
        }
        ranks.sort_unstable_by(|a, b| b.cmp(a));
        ranks.dedup();
        let (properties, count) = body.order();
        let sort = Sort {
            variants: ranks,
            properties,
            count,
            name: class.to_owned(),
        };
        let suffix = if important { " !important" } else { "" };
        let pairs = |pairs: &Pairs| -> Pairs {
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), format!("{value}{suffix}")))
                .collect()
        };
        let mut rules = Vec::new();
        for (at, selectors) in &branches {
            if !body.declarations.is_empty() {
                let mut declarations = pairs(&body.declarations);
                if content && !declarations.iter().any(|(key, _)| key == "content") {
                    declarations.insert(0, ("content".into(), "var(--tw-content)".into()));
                }
                rules.push((at.clone(), selectors.join(","), declarations));
            }
            for (nested, template, values) in &body.nested {
                let mut at = at.clone();
                at.extend(nested.iter().cloned());
                let list: Vec<_> = selectors
                    .iter()
                    .map(|selector| template.replace('&', selector))
                    .collect();
                rules.push((at, list.join(","), pairs(values)));
            }
        }
        Some(Compiled { sort, rules })
    }

    pub(super) fn custom(&self, class: &str) -> Option<Body> {
        let (candidate, modifier) = modifier(class);
        self.custom_with(candidate, modifier).or_else(|| {
            // A value like `16/9` can be one `--value(ratio)` instead of a value and a modifier.
            modifier
                .filter(|modifier| integer(modifier))
                .and_then(|_| self.custom_with(class, None))
        })
    }

    pub(super) fn custom_with(&self, candidate: &str, modifier: Option<&str>) -> Option<Body> {
        let class = candidate;
        self.utilities
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, (name, body))| {
                let value = match name.strip_suffix('*') {
                    Some(prefix) => Some(
                        candidate
                            .strip_prefix(prefix)
                            .filter(|value| !value.is_empty())?,
                    ),
                    None if name == class && modifier.is_none() => None,
                    None => return None,
                };
                let key = (index, value.map(str::to_owned), modifier.map(str::to_owned));
                if let Some(body) = self.expanded.borrow().get(&key) {
                    return body.clone();
                }
                // Bound recursive @apply expansion to stop cycles and deep chains.
                let expanding = self.expanding.borrow().len();
                if expanding > 16 || self.expanding.borrow().contains(&index) {
                    self.cut.set(true);
                    return None;
                }
                let outer = self.cut.replace(false);
                self.expanding.borrow_mut().push(index);
                let mut resolved = [false; 2];
                let body = self
                    .expand(body, value, modifier, &mut resolved, 0)
                    .filter(|_| {
                        (value.is_none() || resolved[0]) && (modifier.is_none() || resolved[1])
                    });
                self.expanding.borrow_mut().truncate(expanding);
                if !self.cut.get() {
                    let mut expanded = self.expanded.borrow_mut();
                    if expanded.len() >= 4096 {
                        expanded.clear();
                    }
                    expanded.insert(key, body.clone());
                }
                self.cut.set(outer || self.cut.get());
                body
            })
    }

    pub(super) fn expand(
        &self,
        source: &str,
        value: Option<&str>,
        modifier: Option<&str>,
        resolved: &mut [bool; 2],
        depth: u8,
    ) -> Option<Body> {
        if depth > 8 {
            return None;
        }
        let mut body = Body::default();
        // Cap @apply output: multiple utilities can multiply at every level.
        let mut size = 0;
        let weigh = |pairs: &Pairs| {
            pairs
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
        };
        for (prelude, block) in statements(source) {
            if size > 1 << 16 {
                return None;
            }
            let Some(inner) = block else {
                if let Some(classes) = prelude.strip_prefix("@apply") {
                    let mut compiled: Vec<_> = classes
                        .split_ascii_whitespace()
                        .filter_map(|class| self.compile(class, "&"))
                        .collect();
                    compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                    for (at, selector, pairs) in compiled.into_iter().flat_map(|c| c.rules) {
                        size += weigh(&pairs) + selector.len();
                        if at.is_empty() && selector == "&" {
                            body.declarations.extend(pairs);
                        } else {
                            body.nested.push((at, selector, pairs));
                        }
                    }
                    continue;
                }
                let Some((property, raw)) = prelude.split_once(':') else {
                    continue;
                };
                let mut raw = raw.trim().to_owned();
                let mut valid = true;
                // Never scan substituted text again: it comes from the class and could hold more calls.
                let mut from = 0;
                while let Some(((start, open, close), function, input)) =
                    [("--value", value), ("--modifier", modifier)]
                        .into_iter()
                        .filter_map(|(function, input)| {
                            Some((call(&raw[from..], function)?, function, input))
                        })
                        .min_by_key(|((start, ..), ..)| *start)
                {
                    let (start, open, close) = (from + start, from + open, from + close);
                    let result = (close < raw.len())
                        .then(|| input.and_then(|input| self.resolve(&raw[open..close], input)))
                        .flatten();
                    let Some(result) = result else {
                        valid = false;
                        break;
                    };
                    raw.replace_range(start..=close, &result);
                    from = start + result.len();
                    resolved[usize::from(function == "--modifier")] = true;
                }
                if valid {
                    body.declarations
                        .push((property.trim().to_owned(), self.functions(&raw)));
                }
                continue;
            };
            let inner = self.expand(inner, value, modifier, resolved, depth + 1)?;
            let (at, selector) = if prelude.starts_with('@') {
                (vec![prelude.to_owned()], "&".to_owned())
            } else if prelude.contains('&') {
                (Vec::new(), prelude.to_owned())
            } else {
                (Vec::new(), format!("& {prelude}"))
            };
            if !inner.declarations.is_empty() {
                size += weigh(&inner.declarations) + selector.len();
                body.nested
                    .push((at.clone(), selector.clone(), inner.declarations));
            }
            for (nested, template, pairs) in inner.nested {
                size +=
                    weigh(&pairs) + template.len() + template.matches('&').count() * selector.len();
                if size > 1 << 16 {
                    return None;
                }
                let mut rules = at.clone();
                rules.extend(nested);
                body.nested
                    .push((rules, template.replace('&', &selector), pairs));
            }
        }
        (size <= 1 << 16).then_some(body)
    }

    pub(super) fn utility(&self, class: &str) -> Option<Body> {
        let custom = self.custom(class);
        // Like Tailwind, a custom utility with the name of a built-in one adds to it, in sort order.
        let Some(builtin) = self.builtin(class) else {
            return custom;
        };
        let Some(custom) = custom else {
            return Some(builtin);
        };
        let (mut body, last) = if custom.order() < builtin.order() {
            (custom, builtin)
        } else {
            (builtin, custom)
        };
        body.declarations
            .retain(|(key, _)| last.declarations.iter().all(|(other, _)| other != key));
        body.declarations.extend(last.declarations);
        body.nested.extend(last.nested);
        Some(body)
    }

    pub(super) fn builtin(&self, class: &str) -> Option<Body> {
        if let Some(raw) = class
            .strip_prefix('[')
            .and_then(|raw| raw.strip_suffix(']'))
        {
            let (property, value) = raw.split_once(':')?;
            if !property.starts_with(|c: char| c == '-' || c.is_ascii_lowercase())
                || !property
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            let value = self.functions(&decode(value));
            return (valid(&value) && !value.trim().is_empty())
                .then(|| Body::new(&[(property, value)]))
                .flatten();
        }
        if let Ok(index) = STATIC.binary_search_by(|(name, _)| (*name).cmp(class)) {
            return Some(Body::parse(STATIC[index].1));
        }
        let (negative, class) = match class.strip_prefix('-') {
            Some(class) => (true, class),
            None => (false, class),
        };
        let (class, modifier) = modifier(class);
        if modifier
            .is_some_and(|modifier| !named_value(modifier) && !modifier.starts_with(['[', '(']))
        {
            return None;
        }
        let functional = |root: &str, value: Option<&str>| {
            (!negative || NEGATIVE.contains(&root))
                .then(|| self.functional(root, value, modifier, negative))
                .flatten()
        };
        if matches!(
            class,
            "bg-glass" | "bg-glass-clear" | "bg-glass-interactive"
        ) {
            return None;
        }
        if let Some(value) = class.strip_prefix("bg-glass-") {
            return functional("bg-glass", Some(value));
        }
        if class.ends_with(']') || class.ends_with(')') {
            let open = if class.ends_with(']') { "-[" } else { "-(" };
            let index = class.find(open)?;
            return functional(&class[..index], Some(&class[index + 1..]));
        }
        // Scan valid suffix boundaries once: checking each suffix again is quadratic.
        let named = class
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '%' | '-')))
            .map_or(0, |index| index + 1);
        functional(class, None).or_else(|| {
            class
                .match_indices('-')
                .rev()
                .take_while(|(index, _)| *index >= named)
                .filter(|(index, _)| *index > 0 && index + 1 < class.len())
                .find_map(|(index, _)| functional(&class[..index], Some(&class[index + 1..])))
        })
    }

    pub(super) fn functional(
        &self,
        root: &str,
        value: Option<&str>,
        modifier: Option<&str>,
        negative: bool,
    ) -> Option<Body> {
        let negate = |value: String| {
            if !negative {
                return value;
            }
            if let Some(number) = value.strip_prefix("calc(var(--spacing) * ") {
                return format!("calc(var(--spacing) * -{number}");
            }
            match value.as_str() {
                "var(--spacing)" => "calc(var(--spacing) * -1)".into(),
                _ => format!("calc({value} * -1)"),
            }
        };
        let typed = value.and_then(|value| {
            value
                .starts_with(['[', '('])
                .then(|| self.typed(value))
                .flatten()
        });
        let raw = typed.as_ref().map(|(_, raw)| raw.clone());
        let hint = typed.as_ref().and_then(|(hint, _)| hint.as_deref());
        let bare = value.filter(|value| !value.starts_with(['[', '(']));
        let spaced = |value: String| match bare {
            Some("0") => value,
            Some("px") if negative => "-1px".into(),
            Some("full") if negative => "-100%".into(),
            _ => negate(value),
        };
        if value.is_some_and(|value| value.starts_with(['[', '('])) && typed.is_none() {
            return None;
        }
        let fraction = |spaces: bool| {
            let (value, modifier) = (bare?, modifier?);
            (integer(value) && integer(modifier) && modifier != "0").then(|| {
                if spaces {
                    format!("calc({value} / {modifier} * 100%)")
                } else {
                    format!("calc({value}/{modifier} * 100%)")
                }
            })
        };
        let number = |unit: &str| {
            bare.filter(|value| integer(value))
                .map(|value| format!("{value}{unit}"))
        };
        let width = |default: &str| match value {
            None => Some(default.to_owned()),
            Some(_) => match hint {
                Some("length" | "line-width" | "number" | "percentage") => raw.clone(),
                Some(_) => None,
                None => match &raw {
                    Some(raw) => is_length(raw).then(|| raw.clone()),
                    None => bare
                        .filter(|value| integer(value))
                        .map(|value| format!("{value}px")),
                },
            },
        };
        let filter = |name: &str, unit: &str, default: Option<&str>| {
            let value = match (value, &raw) {
                (None, _) => default?.to_owned(),
                (_, Some(raw)) => raw.clone(),
                _ if root == "backdrop-opacity" => self.opacity(bare?)?,
                _ => number(unit)?,
            };
            let backdrop = root.starts_with("backdrop-");
            let property = format!("--tw-{root}");
            let value = format!("{name}({})", negate(value));
            if backdrop {
                return Body::new(&[
                    (&property, value),
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ]);
            }
            Body::new(&[(&property, value), ("filter", FILTER.into())])
        };
        if modifier.is_some()
            && !matches!(
                root,
                "w" | "min-w"
                    | "max-w"
                    | "h"
                    | "min-h"
                    | "max-h"
                    | "size"
                    | "inset"
                    | "inset-x"
                    | "inset-y"
                    | "inset-s"
                    | "inset-e"
                    | "inset-bs"
                    | "inset-be"
                    | "start"
                    | "end"
                    | "top"
                    | "right"
                    | "bottom"
                    | "left"
                    | "translate"
                    | "translate-x"
                    | "translate-y"
                    | "inline"
                    | "min-inline"
                    | "max-inline"
                    | "block"
                    | "min-block"
                    | "max-block"
                    | "basis"
                    | "flex"
                    | "aspect"
                    | "text"
                    | "bg"
                    | "bg-glass"
                    | "bg-linear"
                    | "bg-radial"
                    | "bg-conic"
                    | "from"
                    | "via"
                    | "to"
                    | "border"
                    | "border-x"
                    | "border-y"
                    | "border-s"
                    | "border-e"
                    | "border-bs"
                    | "border-be"
                    | "border-t"
                    | "border-r"
                    | "border-b"
                    | "border-l"
                    | "divide"
                    | "outline"
                    | "ring"
                    | "inset-ring"
                    | "ring-offset"
                    | "shadow"
                    | "inset-shadow"
                    | "drop-shadow"
                    | "text-shadow"
                    | "decoration"
                    | "accent"
                    | "caret"
                    | "fill"
                    | "stroke"
                    | "placeholder"
                    | "scrollbar-thumb"
                    | "scrollbar-track"
                    | "mask-linear-from"
                    | "mask-linear-to"
                    | "mask-radial-from"
                    | "mask-radial-to"
                    | "mask-conic-from"
                    | "mask-conic-to"
                    | "mask-x-from"
                    | "mask-x-to"
                    | "mask-y-from"
                    | "mask-y-to"
                    | "mask-t-from"
                    | "mask-t-to"
                    | "mask-r-from"
                    | "mask-r-to"
                    | "mask-b-from"
                    | "mask-b-to"
                    | "mask-l-from"
                    | "mask-l-to"
                    | "@container"
            )
        {
            return None;
        }
        match root {
            "w" | "min-w" | "max-w" | "h" | "min-h" | "max-h" | "size" | "inline"
            | "min-inline" | "max-inline" | "block" | "min-block" | "max-block" => {
                let value = value?;
                let max = root.starts_with("max");
                let vertical = root.ends_with("block") || root.ends_with('h');
                let inline = root.ends_with("inline");
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "full" => "100%".into(),
                        "auto" if !max => "auto".into(),
                        "none" if max => "none".into(),
                        "px" => "1px".into(),
                        "screen" if root != "size" && root != "max-w" => {
                            if vertical { "100vh" } else { "100vw" }.into()
                        }
                        "dvw" | "svw" | "lvw" if !root.ends_with("block") => format!("100{value}"),
                        "dvh" | "svh" | "lvh" if !inline => format!("100{value}"),
                        "min" => "min-content".into(),
                        "max" => "max-content".into(),
                        "fit" => "fit-content".into(),
                        "lh" if vertical => "1lh".into(),
                        "prose" if root == "max-w" => "65ch".into(),
                        _ => (!vertical && root != "size")
                            .then(|| self.theme("--container", value))
                            .flatten()
                            .or_else(|| {
                                (root == "max-w")
                                    .then(|| {
                                        self.theme("--breakpoint", value.strip_prefix("screen-")?)
                                    })
                                    .flatten()
                            })
                            .or_else(|| self.spacing(value))?,
                    },
                };
                let properties: &[&str] = match root {
                    "w" => &["width"],
                    "min-w" => &["min-width"],
                    "max-w" => &["max-width"],
                    "h" => &["height"],
                    "min-h" => &["min-height"],
                    "max-h" => &["max-height"],
                    "size" => &["width", "height"],
                    "inline" => &["inline-size"],
                    "min-inline" => &["min-inline-size"],
                    "max-inline" => &["max-inline-size"],
                    "block" => &["block-size"],
                    "min-block" => &["min-block-size"],
                    _ => &["max-block-size"],
                };
                Body::new(
                    &properties
                        .iter()
                        .map(|property| (*property, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "inset" | "inset-x" | "inset-y" | "inset-s" | "inset-e" | "inset-bs" | "inset-be"
            | "start" | "end" | "top" | "right" | "bottom" | "left" => {
                let value = value?;
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "auto" if !negative => "auto".into(),
                        "full" => "100%".into(),
                        // Tailwind keeps the old spacing for the deprecated `start-*` and `end-*`.
                        _ if matches!(root, "start" | "end") && value != "px" => {
                            let base = self.function("--spacing", "1")?;
                            self.spacing(value)?;
                            let sign = if negative { "-" } else { "" };
                            return Body::new(&[(
                                if root == "start" {
                                    "inset-inline-start"
                                } else {
                                    "inset-inline-end"
                                },
                                format!("calc({base} * {sign}{value})"),
                            )]);
                        }
                        _ => self.spacing(value)?,
                    },
                };
                let resolved = if raw.is_some() || modifier.is_some() {
                    negate(resolved)
                } else {
                    spaced(resolved)
                };
                let property = match root {
                    "inset-x" => "inset-inline",
                    "inset-y" => "inset-block",
                    "start" | "inset-s" => "inset-inline-start",
                    "end" | "inset-e" => "inset-inline-end",
                    "inset-bs" => "inset-block-start",
                    "inset-be" => "inset-block-end",
                    _ => root,
                };
                Body::new(&[(property, resolved)])
            }
            "m" | "mx" | "my" | "ms" | "me" | "mbs" | "mbe" | "mt" | "mr" | "mb" | "ml" | "p"
            | "px" | "py" | "ps" | "pe" | "pbs" | "pbe" | "pt" | "pr" | "pb" | "pl"
            | "scroll-m" | "scroll-mx" | "scroll-my" | "scroll-ms" | "scroll-me" | "scroll-mbs"
            | "scroll-mbe" | "scroll-mt" | "scroll-mr" | "scroll-mb" | "scroll-ml" | "scroll-p"
            | "scroll-px" | "scroll-py" | "scroll-ps" | "scroll-pe" | "scroll-pbs"
            | "scroll-pbe" | "scroll-pt" | "scroll-pr" | "scroll-pb" | "scroll-pl" => {
                let value = value?;
                let (scroll, short) = match root.strip_prefix("scroll-") {
                    Some(short) => ("scroll-", short),
                    None => ("", root),
                };
                let margin = short.starts_with('m');
                let resolved = match value {
                    "auto" if margin && !negative && scroll.is_empty() => "auto".into(),
                    _ => self.spacing(value)?,
                };
                let name = if margin { "margin" } else { "padding" };
                let side = match &short[1..] {
                    "" => "",
                    "x" => "-inline",
                    "y" => "-block",
                    "s" => "-inline-start",
                    "e" => "-inline-end",
                    "bs" => "-block-start",
                    "be" => "-block-end",
                    "t" => "-top",
                    "r" => "-right",
                    "b" => "-bottom",
                    _ => "-left",
                };
                Body::new(&[(&format!("{scroll}{name}{side}"), spaced(resolved))])
            }
            "gap" | "gap-x" | "gap-y" => {
                let property = match root {
                    "gap-x" => "column-gap",
                    "gap-y" => "row-gap",
                    _ => "gap",
                };
                Body::new(&[(property, self.spacing(value?)?)])
            }
            "space-x" | "space-y" => {
                let value = spaced(self.spacing(value?)?);
                let axis = &root[6..];
                let (start, end) = if axis == "x" {
                    ("margin-inline-start", "margin-inline-end")
                } else {
                    ("margin-block-start", "margin-block-end")
                };
                let reverse = format!("--tw-space-{axis}-reverse");
                let (start_value, end_value) = if value == "0px" {
                    ("0".into(), "0".into())
                } else {
                    (
                        format!("calc({value} * var({reverse}))"),
                        format!("calc({value} * calc(1 - var({reverse})))"),
                    )
                };
                Some(Body {
                    nested: vec![(
                        Vec::new(),
                        ":where(& > :not(:last-child))".into(),
                        vec![
                            (reverse, "0".into()),
                            (start.into(), start_value),
                            (end.into(), end_value),
                        ],
                    )],
                    sort: Some(if axis == "x" { "column-gap" } else { "row-gap" }),
                    ..Body::default()
                })
            }
            "space-x-reverse" | "space-y-reverse" | "divide-x-reverse" | "divide-y-reverse"
                if value.is_none() =>
            {
                let name = root.strip_suffix("-reverse")?;
                Some(Body::default().nest(
                    &[],
                    ":where(& > :not(:last-child))",
                    &format!("--tw-{name}-reverse:1"),
                ))
            }
            "divide" | "divide-x" | "divide-y" => {
                let nested = ":where(& > :not(:last-child))";
                if root == "divide" {
                    if let Some(style) = bare.filter(|style| {
                        matches!(*style, "solid" | "dashed" | "dotted" | "double" | "none")
                    }) {
                        return Some(Body::default().nest(
                            &[],
                            nested,
                            &format!("--tw-border-style:{style};border-style:{style}"),
                        ));
                    }
                    let color = self.color(value?, modifier)?;
                    return Some(Body::default().nest(
                        &[],
                        nested,
                        &format!("border-color:{color}"),
                    ));
                }
                let width = raw.clone().or_else(|| width("1px"))?;
                let axis = &root[7..];
                let reverse = format!("--tw-divide-{axis}-reverse");
                let source = if axis == "x" {
                    format!(
                        "{reverse}:0;border-inline-style:var(--tw-border-style);border-inline-start-width:calc({width} * var({reverse}));border-inline-end-width:calc({width} * calc(1 - var({reverse})))"
                    )
                } else {
                    format!(
                        "{reverse}:0;border-bottom-style:var(--tw-border-style);border-top-style:var(--tw-border-style);border-top-width:calc({width} * var({reverse}));border-bottom-width:calc({width} * calc(1 - var({reverse})))"
                    )
                };
                let mut body = Body::default().nest(&[], nested, &source);
                body.sort = Some(if axis == "x" {
                    "divide-x-width"
                } else {
                    "divide-y-width"
                });
                Some(body)
            }
            "translate" | "translate-x" | "translate-y" | "translate-z" => {
                let value = value?;
                if root == "translate" && bare == Some("none") {
                    return Body::new(&[("translate", "none".into())]);
                }
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => negate(raw.clone()),
                    (_, Some(_)) => negate(fraction(true)?),
                    _ if value == "full" => spaced("100%".into()),
                    _ => spaced(self.spacing(value)?),
                };
                let axes: &[&str] = match root {
                    "translate" => &["x", "y"],
                    "translate-x" => &["x"],
                    "translate-y" => &["y"],
                    _ => &["z"],
                };
                let mut body: Pairs = axes
                    .iter()
                    .map(|axis| (format!("--tw-translate-{axis}"), resolved.clone()))
                    .collect();
                body.push((
                    "translate".into(),
                    if root == "translate-z" {
                        "var(--tw-translate-x) var(--tw-translate-y) var(--tw-translate-z)"
                    } else {
                        "var(--tw-translate-x) var(--tw-translate-y)"
                    }
                    .into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "scale" | "scale-x" | "scale-y" | "scale-z" => {
                let value = value?;
                if root == "scale"
                    && let Some(raw) = &raw
                {
                    return Body::new(&[("scale", negate(raw.clone()))]);
                }
                let resolved = negate(match &raw {
                    Some(raw) => raw.clone(),
                    None => integer(value).then(|| format!("{value}%"))?,
                });
                let axes: &[&str] = match root {
                    "scale" => &["x", "y", "z"],
                    "scale-x" => &["x"],
                    "scale-y" => &["y"],
                    _ => &["z"],
                };
                let mut body: Pairs = axes
                    .iter()
                    .map(|axis| (format!("--tw-scale-{axis}"), resolved.clone()))
                    .collect();
                body.push((
                    "scale".into(),
                    if root == "scale-z" {
                        "var(--tw-scale-x) var(--tw-scale-y) var(--tw-scale-z)"
                    } else {
                        "var(--tw-scale-x) var(--tw-scale-y)"
                    }
                    .into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "rotate" | "rotate-x" | "rotate-y" | "rotate-z" | "skew" | "skew-x" | "skew-y" => {
                let resolved = negate(match &raw {
                    Some(raw) => raw.clone(),
                    None => number("deg")?,
                });
                if root == "rotate" {
                    return Body::new(&[("rotate", resolved)]);
                }
                let mut body: Pairs = match root {
                    "skew" => vec![
                        ("--tw-skew-x".into(), format!("skewX({resolved})")),
                        ("--tw-skew-y".into(), format!("skewY({resolved})")),
                    ],
                    _ => {
                        let axis = &root[root.len() - 1..];
                        let function = if root.starts_with("skew") {
                            "skew"
                        } else {
                            "rotate"
                        };
                        vec![(
                            format!("--tw-{root}"),
                            format!("{function}{}({resolved})", axis.to_ascii_uppercase()),
                        )]
                    }
                };
                body.push(("transform".into(), TRANSFORM.into()));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "transform" => match (value, &raw) {
                (None, _) => Body::new(&[("transform", TRANSFORM.into())]),
                (_, Some(raw)) => Body::new(&[("transform", raw.clone())]),
                _ => None,
            },
            "perspective" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "none".into(),
                    (Some(value), _) => self.theme("--perspective", value)?,
                    _ => return None,
                };
                Body::new(&[("perspective", resolved)])
            }
            "origin" | "perspective-origin" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => ORIGINS
                        .iter()
                        .find(|(name, _)| *name == value)
                        .map(|(_, position)| (*position).to_owned())?,
                    _ => return None,
                };
                let property = if root == "origin" {
                    "transform-origin"
                } else {
                    root
                };
                Body::new(&[(property, resolved)])
            }
            "z" | "order" | "col" | "col-start" | "col-end" | "row" | "row-start" | "row-end"
            | "col-span" | "row-span" | "columns" | "tab" | "line-clamp" | "grow" | "shrink"
            | "flex-grow" | "flex-shrink" => {
                let span = root.ends_with("-span");
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) if span => format!("span {raw} / span {raw}"),
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) if negative && !integer(value) => return None,
                    (None, _)
                        if matches!(root, "grow" | "shrink" | "flex-grow" | "flex-shrink") =>
                    {
                        "1".into()
                    }
                    (Some("auto"), _)
                        if matches!(
                            root,
                            "z" | "col"
                                | "col-start"
                                | "col-end"
                                | "row"
                                | "row-start"
                                | "row-end"
                                | "columns"
                        ) =>
                    {
                        "auto".into()
                    }
                    (Some("first"), _) if root == "order" => "-9999".into(),
                    (Some("last"), _) if root == "order" => "9999".into(),
                    (Some("none"), _) if root == "order" => "0".into(),
                    (Some("full"), _) if span => "1 / -1".into(),
                    (Some(value), _) if integer(value) => {
                        if span {
                            format!("span {value} / span {value}")
                        } else {
                            value.to_owned()
                        }
                    }
                    (Some(value), _) if root == "columns" => self.theme("--container", value)?,
                    _ => return None,
                };
                if root == "line-clamp" {
                    return Body::new(&[
                        ("overflow", "hidden".into()),
                        ("display", "-webkit-box".into()),
                        ("-webkit-box-orient", "vertical".into()),
                        ("-webkit-line-clamp", resolved),
                    ]);
                }
                let property = match root {
                    "z" => "z-index",
                    "col" | "col-span" => "grid-column",
                    "col-start" => "grid-column-start",
                    "col-end" => "grid-column-end",
                    "row" | "row-span" => "grid-row",
                    "row-start" => "grid-row-start",
                    "row-end" => "grid-row-end",
                    "tab" => "tab-size",
                    "grow" | "flex-grow" => "flex-grow",
                    "shrink" | "flex-shrink" => "flex-shrink",
                    _ => root,
                };
                Body::new(&[(property, negate(resolved))])
            }
            "grid-cols" | "grid-rows" | "auto-cols" | "auto-rows" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none" | "subgrid"), _) if root.starts_with("grid") => bare?.into(),
                    (Some(value), _)
                        if root.starts_with("grid") && integer(value) && value != "0" =>
                    {
                        format!("repeat({value}, minmax(0, 1fr))")
                    }
                    (Some("auto"), _) if root.starts_with("auto") => "auto".into(),
                    (Some("min"), _) if root.starts_with("auto") => "min-content".into(),
                    (Some("max"), _) if root.starts_with("auto") => "max-content".into(),
                    (Some("fr"), _) if root.starts_with("auto") => "minmax(0, 1fr)".into(),
                    (Some(value), _) if root.starts_with("auto") && value != "px" => {
                        self.spacing(value)?
                    }
                    _ => return None,
                };
                let property = match root {
                    "grid-cols" => "grid-template-columns",
                    "grid-rows" => "grid-template-rows",
                    "auto-cols" => "grid-auto-columns",
                    _ => "grid-auto-rows",
                };
                Body::new(&[(property, resolved)])
            }
            "flex" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    _ if modifier.is_some() => fraction(false)?,
                    (Some(value), _) if integer(value) => value.to_owned(),
                    _ => return None,
                };
                Body::new(&[("flex", resolved)])
            }
            "basis" => {
                let value = value?;
                let resolved = match (&raw, modifier) {
                    (Some(raw), None) => raw.clone(),
                    (_, Some(_)) => fraction(false)?,
                    _ => match value {
                        "full" => "100%".into(),
                        "auto" => "auto".into(),
                        _ => self
                            .theme("--container", value)
                            .or_else(|| self.spacing(value))?,
                    },
                };
                Body::new(&[("flex-basis", resolved)])
            }
            "border-spacing" | "border-spacing-x" | "border-spacing-y" => {
                let resolved = self.spacing(value?)?;
                let mut body: Pairs = match root {
                    "border-spacing-x" => vec![("--tw-border-spacing-x".into(), resolved)],
                    "border-spacing-y" => vec![("--tw-border-spacing-y".into(), resolved)],
                    _ => vec![
                        ("--tw-border-spacing-x".into(), resolved.clone()),
                        ("--tw-border-spacing-y".into(), resolved),
                    ],
                };
                body.push((
                    "border-spacing".into(),
                    "var(--tw-border-spacing-x) var(--tw-border-spacing-y)".into(),
                ));
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "indent" => Body::new(&[("text-indent", spaced(self.spacing(value?)?))]),
            _ if root == "rounded" || root.starts_with("rounded-") => {
                let corners: &[&str] = match root.strip_prefix("rounded-").unwrap_or("") {
                    "" => &["border-radius"],
                    "t" => &["border-top-left-radius", "border-top-right-radius"],
                    "r" => &["border-top-right-radius", "border-bottom-right-radius"],
                    "b" => &["border-bottom-right-radius", "border-bottom-left-radius"],
                    "l" => &["border-top-left-radius", "border-bottom-left-radius"],
                    "s" => &["border-start-start-radius", "border-end-start-radius"],
                    "e" => &["border-start-end-radius", "border-end-end-radius"],
                    "tl" => &["border-top-left-radius"],
                    "tr" => &["border-top-right-radius"],
                    "br" => &["border-bottom-right-radius"],
                    "bl" => &["border-bottom-left-radius"],
                    "ss" => &["border-start-start-radius"],
                    "se" => &["border-start-end-radius"],
                    "ee" => &["border-end-end-radius"],
                    "es" => &["border-end-start-radius"],
                    _ => return None,
                };
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (None, _) => self.raw("--radius")?,
                    (Some("none"), _) => "0".into(),
                    (Some("full"), _) => "calc(infinity * 1px)".into(),
                    (Some(value), _) => self.theme("--radius", value)?,
                };
                Body::new(
                    &corners
                        .iter()
                        .map(|corner| (*corner, resolved.clone()))
                        .collect::<Vec<_>>(),
                )
            }
            "border" | "border-x" | "border-y" | "border-s" | "border-e" | "border-bs"
            | "border-be" | "border-t" | "border-r" | "border-b" | "border-l" => {
                let side = match &root[6..] {
                    "" => "",
                    "-x" => "-inline",
                    "-y" => "-block",
                    "-s" => "-inline-start",
                    "-e" => "-inline-end",
                    "-bs" => "-block-start",
                    "-be" => "-block-end",
                    "-t" => "-top",
                    "-r" => "-right",
                    "-b" => "-bottom",
                    _ => "-left",
                };
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        (
                            &format!("border{side}-style"),
                            "var(--tw-border-style)".into(),
                        ),
                        (&format!("border{side}-width"), width),
                    ]);
                }
                Body::new(&[(
                    &format!("border{side}-color"),
                    self.color(value?, modifier)?,
                )])
            }
            "bg" => {
                let value = value?;
                if let Some(raw) = &raw {
                    let property = match hint {
                        Some("image" | "url") => Some("background-image"),
                        Some("length" | "size" | "bg-size") => Some("background-size"),
                        Some("position" | "percentage") => Some("background-position"),
                        Some("color") => None,
                        Some(_) => return None,
                        None if raw.starts_with("url(") || raw.contains("gradient(") => {
                            Some("background-image")
                        }
                        None if is_color(raw) || raw.starts_with("var(") => None,
                        None => layer(raw).map(|kind| {
                            if kind == "size" {
                                "background-size"
                            } else {
                                "background-position"
                            }
                        }),
                    };
                    if let Some(property) = property {
                        return Body::new(&[(property, raw.clone())]);
                    }
                }
                Body::new(&[("background-color", self.color(value, modifier)?)])
            }
            "bg-size" | "bg-position" | "mask-size" | "mask-position" => {
                let property = match root {
                    "bg-size" => "background-size",
                    "bg-position" => "background-position",
                    _ => root,
                };
                Body::new(&[(property, raw?)])
            }
            "bg-linear" | "bg-gradient-to" | "bg-radial" | "bg-conic" => {
                let interpolation = match modifier {
                    None => "in oklab".into(),
                    Some(modifier) if modifier.starts_with(['[', '(']) => {
                        self.arbitrary(modifier)?
                    }
                    Some(modifier @ ("longer" | "shorter" | "increasing" | "decreasing")) => {
                        format!("in oklch {modifier} hue")
                    }
                    Some(modifier @ ("srgb" | "hsl" | "oklab" | "oklch")) => {
                        format!("in {modifier}")
                    }
                    _ => return None,
                };
                let kind = match root {
                    "bg-radial" => "radial",
                    "bg-conic" => "conic",
                    _ => "linear",
                };
                if let Some(raw) = &raw {
                    return Body::new(&[
                        ("--tw-gradient-position", raw.clone()),
                        (
                            "background-image",
                            format!("{kind}-gradient(var(--tw-gradient-stops,{raw}))"),
                        ),
                    ]);
                }
                let position = match (root, bare) {
                    ("bg-gradient-to", Some(side)) => {
                        let side = direction(side)?;
                        return Body::new(&[
                            ("--tw-gradient-position", format!("to {side} in oklab")),
                            (
                                "background-image",
                                "linear-gradient(var(--tw-gradient-stops))".into(),
                            ),
                        ]);
                    }
                    ("bg-linear", Some(side)) if side.starts_with("to-") => {
                        format!("to {}", direction(&side[3..])?)
                    }
                    ("bg-linear", Some(angle)) => {
                        negate(format!("{}deg", integer(angle).then_some(angle)?))
                    }
                    ("bg-conic", Some(angle)) => {
                        format!(
                            "from {}",
                            negate(format!("{}deg", integer(angle).then_some(angle)?))
                        )
                    }
                    ("bg-radial" | "bg-conic", None) => String::new(),
                    _ => return None,
                };
                let image = (
                    "background-image",
                    format!("{kind}-gradient(var(--tw-gradient-stops))"),
                );
                if kind == "linear" {
                    let mut body = Body::new(&[("--tw-gradient-position", position.clone())])?;
                    body = body.nest(
                        &["@supports (background-image: linear-gradient(in lab, red, red))"],
                        "&",
                        &format!("--tw-gradient-position:{position} {interpolation}"),
                    );
                    body.nested
                        .push((Vec::new(), "&".into(), vec![(image.0.into(), image.1)]));
                    return Some(body);
                }
                let position = if position.is_empty() {
                    interpolation
                } else {
                    format!("{position} {interpolation}")
                };
                Body::new(&[("--tw-gradient-position", position), image])
            }
            "from" | "via" | "to" => {
                let value = value?;
                let position = match (bare, hint, &raw) {
                    (Some(value), ..) if value.ends_with('%') => {
                        let number = value.strip_suffix('%')?;
                        integer(number).then(|| value.to_owned())
                    }
                    (_, Some("length" | "percentage"), Some(raw)) => Some(raw.clone()),
                    (_, None, Some(raw)) if is_length(raw) => Some(raw.clone()),
                    _ => None,
                };
                if let Some(position) = position {
                    return Body::new(&[(&format!("--tw-gradient-{root}-position"), position)]);
                }
                let color = self.color(value, modifier)?;
                if root == "via" {
                    return Body::new(&[
                        ("--tw-gradient-via", color),
                        ("--tw-gradient-via-stops", "var(--tw-gradient-position), var(--tw-gradient-from) var(--tw-gradient-from-position), var(--tw-gradient-via) var(--tw-gradient-via-position), var(--tw-gradient-to) var(--tw-gradient-to-position)".into()),
                        ("--tw-gradient-stops", "var(--tw-gradient-via-stops)".into()),
                    ]);
                }
                Body::new(&[
                    (&format!("--tw-gradient-{root}"), color),
                    ("--tw-gradient-stops", STOPS.into()),
                ])
            }
            "mask" => {
                let raw = raw?;
                let kind = match hint {
                    Some("position" | "percentage") => Some("position"),
                    Some("length" | "size" | "bg-size") => Some("size"),
                    Some(_) => None,
                    None => layer(&raw),
                };
                Body::new(&[(
                    match kind {
                        Some("position") => "mask-position",
                        Some(_) => "mask-size",
                        None => "mask-image",
                    },
                    raw,
                )])
            }
            "mask-linear" | "mask-conic" | "mask-radial" => {
                let kind = &root[5..];
                let position = match (bare, &raw) {
                    (_, Some(raw)) => negate(raw.clone()),
                    (Some(angle @ ("0" | "1")), None) if kind != "radial" => {
                        format!(
                            "{}{angle}deg",
                            if negative && angle == "1" { "-" } else { "" }
                        )
                    }
                    (Some(angle), None) if kind != "radial" && integer(angle) => {
                        format!("calc(1deg * {}{angle})", if negative { "-" } else { "" })
                    }
                    _ => return None,
                };
                let name = if kind == "radial" { "size" } else { "position" };
                let mut body = Body::parse(MASK);
                body.declarations.push((
                    format!("--tw-mask-{kind}"),
                    format!(
                        "{kind}-gradient(var(--tw-mask-{kind}-stops, var(--tw-mask-{kind}-{name})))"
                    ),
                ));
                body.declarations
                    .push((format!("--tw-mask-{kind}-{name}"), position));
                Some(body)
            }
            _ if root.starts_with("mask-")
                && (root.ends_with("-from") || root.ends_with("-to")) =>
            {
                let (edge, end) = root[5..].rsplit_once('-')?;
                let value = value?;
                let position = match (bare, hint, &raw) {
                    (_, Some("color"), _) => None,
                    (_, Some(_), Some(raw)) => Some(raw.clone()),
                    (_, None, Some(raw)) if !is_color(raw) => Some(raw.clone()),
                    (Some(value), ..) if value.ends_with('%') => {
                        integer(value.strip_suffix('%')?).then(|| value.to_owned())
                    }
                    (Some(value), ..) => self.spacing(value),
                    _ => None,
                };
                let (field, assigned) = match position {
                    Some(position) => ("position", position),
                    None => ("color", self.color(value, modifier)?),
                };
                let mut body = Body::parse(MASK);
                let mut push = |name: String, value: String| body.declarations.push((name, value));
                match edge {
                    "linear" | "radial" | "conic" => {
                        let stops = match edge {
                            "linear" => "var(--tw-mask-linear-position), ",
                            "radial" => {
                                "var(--tw-mask-radial-shape) var(--tw-mask-radial-size) at var(--tw-mask-radial-position), "
                            }
                            _ => "from var(--tw-mask-conic-position), ",
                        };
                        push(
                            format!("--tw-mask-{edge}-stops"),
                            format!(
                                "{stops}var(--tw-mask-{edge}-from-color) var(--tw-mask-{edge}-from-position), var(--tw-mask-{edge}-to-color) var(--tw-mask-{edge}-to-position)"
                            ),
                        );
                        push(
                            format!("--tw-mask-{edge}"),
                            format!("{edge}-gradient(var(--tw-mask-{edge}-stops))"),
                        );
                        push(format!("--tw-mask-{edge}-{end}-{field}"), assigned);
                    }
                    _ => {
                        let sides: &[&str] = match edge {
                            "x" => &["right", "left"],
                            "y" => &["top", "bottom"],
                            "t" => &["top"],
                            "r" => &["right"],
                            "b" => &["bottom"],
                            "l" => &["left"],
                            _ => return None,
                        };
                        push("--tw-mask-linear".into(), EDGES.into());
                        for side in sides {
                            push(
                                format!("--tw-mask-{side}"),
                                format!(
                                    "linear-gradient(to {side}, var(--tw-mask-{side}-from-color) var(--tw-mask-{side}-from-position), var(--tw-mask-{side}-to-color) var(--tw-mask-{side}-to-position))"
                                ),
                            );
                            push(format!("--tw-mask-{side}-{end}-{field}"), assigned.clone());
                        }
                    }
                }
                Some(body)
            }
            "text" => {
                let value = value?;
                let size = match (hint, &raw) {
                    (
                        Some("length" | "percentage" | "absolute-size" | "relative-size"),
                        Some(raw),
                    ) => Some(raw.clone()),
                    (None, Some(raw))
                        if is_length(raw)
                            || matches!(
                                raw.as_str(),
                                "xx-small"
                                    | "x-small"
                                    | "small"
                                    | "medium"
                                    | "large"
                                    | "x-large"
                                    | "xx-large"
                                    | "xxx-large"
                                    | "larger"
                                    | "smaller"
                            ) =>
                    {
                        Some(raw.clone())
                    }
                    (None, None) => self.theme("--text", value),
                    _ => None,
                };
                let Some(size) = size else {
                    return Body::new(&[("color", self.color(value, modifier)?)]);
                };
                let mut body = vec![("font-size", size)];
                match modifier {
                    Some(modifier) => body.push((
                        "line-height",
                        self.arbitrary(modifier)
                            .or_else(|| self.theme("--leading", modifier))
                            .or_else(|| self.spacing(modifier))
                            .or_else(|| (modifier == "none").then(|| "1".into()))?,
                    )),
                    None if raw.is_none() => {
                        for (property, var) in [
                            ("line-height", "leading"),
                            ("letter-spacing", "tracking"),
                            ("font-weight", "font-weight"),
                        ] {
                            if let Some(option) =
                                self.theme("--text", &format!("{value}--{property}"))
                            {
                                body.push((property, format!("var(--tw-{var}, {option})")));
                            }
                        }
                    }
                    None => {}
                }
                Body::new(&body)
            }
            "font" => {
                if let Some(raw) = &raw {
                    let weight = match hint {
                        Some("family-name" | "generic-name") => false,
                        Some(_) => true,
                        // Tailwind's order: a var() or a number is a weight, then a family name has no
                        // part that starts with a digit and one part that is not a var().
                        None => {
                            let parts = split(raw, ',');
                            raw.starts_with("var(")
                                || raw.parse::<f64>().is_ok()
                                || MATH.iter().any(|name| raw.contains(&format!("{name}(")))
                                || parts
                                    .iter()
                                    .any(|part| part.starts_with(|c: char| c.is_ascii_digit()))
                                || parts.iter().all(|part| part.starts_with("var("))
                        }
                    };
                    if weight {
                        return Body::new(&[
                            ("--tw-font-weight", raw.clone()),
                            ("font-weight", raw.clone()),
                        ]);
                    }
                    return Body::new(&[("font-family", raw.clone())]);
                }
                let value = bare?;
                if let Some(family) = self.theme("--font", value) {
                    return Body::new(&[("font-family", family)]);
                }
                let weight = self.theme("--font-weight", value)?;
                Body::new(&[
                    ("--tw-font-weight", weight.clone()),
                    ("font-weight", weight),
                ])
            }
            "font-stretch" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => {
                        let number = value.strip_suffix('%')?;
                        let number: f64 = number.parse().ok()?;
                        (50. ..=200.).contains(&number).then(|| value.to_owned())?
                    }
                    _ => return None,
                };
                Body::new(&[("font-stretch", resolved)])
            }
            "leading" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "1".into(),
                    (Some(value), _) => self
                        .theme("--leading", value)
                        .or_else(|| self.spacing(value))?,
                    _ => return None,
                };
                Body::new(&[
                    ("--tw-leading", resolved.clone()),
                    ("line-height", resolved),
                ])
            }
            "tracking" => {
                let resolved = negate(match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some(value), _) => self.theme("--tracking", value)?,
                    _ => return None,
                });
                Body::new(&[
                    ("--tw-tracking", resolved.clone()),
                    ("letter-spacing", resolved),
                ])
            }
            "opacity" | "zoom" => {
                let resolved = match &raw {
                    Some(raw) => raw.clone(),
                    None => {
                        let value = bare?;
                        let number: f64 = value.parse().ok()?;
                        (if root == "zoom" {
                            integer(value)
                        } else {
                            (0. ..=100.).contains(&number) && (number * 4.).fract() == 0.
                        })
                        .then(|| format!("{value}%"))?
                    }
                };
                Body::new(&[(root, resolved)])
            }
            "shadow" | "inset-shadow" | "text-shadow" | "drop-shadow" => {
                let shadow = match (bare, &raw) {
                    (None, None) => match root {
                        "shadow" => self.raw("--shadow"),
                        "drop-shadow" => self.raw("--drop-shadow"),
                        _ => None,
                    },
                    (Some("none"), _) => Some("none".into()),
                    (Some("inner"), _) if root == "shadow" => self.raw("--shadow-inner"),
                    (Some(value), _) => self.raw(&format!("--{root}-{value}")),
                    (_, Some(raw)) if hint.is_some_and(|hint| hint != "color") => Some(raw.clone()),
                    (_, Some(raw)) if hint.is_none() && !is_color(raw) => Some(raw.clone()),
                    _ => None,
                };
                let Some(shadow) = shadow else {
                    let color = self.shadow(value?, modifier, root)?;
                    if root == "drop-shadow" {
                        return Body::new(&[
                            ("--tw-drop-shadow-color", color),
                            ("--tw-drop-shadow", "var(--tw-drop-shadow-size)".into()),
                        ]);
                    }
                    return Body::new(&[(&format!("--tw-{root}-color"), color)]);
                };
                let alpha = match modifier {
                    Some(modifier) => Some(self.opacity(modifier)?),
                    None => None,
                };
                let name = format!("--tw-{root}-color");
                let colored = |shadow: &str| shadow_color(shadow, &name, alpha.as_deref());
                let mut body = Vec::new();
                if let Some(alpha) = &alpha {
                    body.push((format!("--tw-{root}-alpha"), alpha.clone()));
                }
                let variable = raw.is_some() && shadow.starts_with("var(");
                match root {
                    "shadow" | "inset-shadow" => {
                        let value = match (root, shadow.as_str()) {
                            ("shadow", "none") => "0 0 #0000".into(),
                            (_, "none") => "inset 0 0 #0000".into(),
                            ("inset-shadow", shadow) if raw.is_some() => {
                                colored(&format!("inset {shadow}"))
                            }
                            (_, shadow) if variable => shadow.to_owned(),
                            (_, shadow) => colored(shadow),
                        };
                        body.push((format!("--tw-{root}"), value));
                        body.push(("box-shadow".into(), SHADOW.into()));
                    }
                    "text-shadow" => {
                        let value = if shadow == "none" || variable {
                            shadow.clone()
                        } else {
                            colored(&shadow)
                        };
                        body.push(("text-shadow".into(), value));
                    }
                    _ if shadow == "none" => {
                        body.push(("--tw-drop-shadow".into(), " ".into()));
                        body.push(("filter".into(), FILTER.into()));
                    }
                    _ => {
                        let layers = |colored: &dyn Fn(&str) -> String| {
                            split(&shadow, ',')
                                .iter()
                                .map(|layer| format!("drop-shadow({})", colored(layer.trim())))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        let size = if variable {
                            format!("drop-shadow({shadow})")
                        } else {
                            layers(&colored)
                        };
                        let plain = match (bare, value) {
                            (Some(value), _) if alpha.is_none() => {
                                format!("drop-shadow(var(--drop-shadow-{value}))")
                            }
                            (None, None) => layers(&|layer: &str| layer.to_owned()),
                            _ => "var(--tw-drop-shadow-size)".into(),
                        };
                        body.push(("--tw-drop-shadow-size".into(), size));
                        body.push(("--tw-drop-shadow".into(), plain));
                        body.push(("filter".into(), FILTER.into()));
                    }
                }
                Some(Body {
                    declarations: body,
                    ..Body::default()
                })
            }
            "ring" | "inset-ring" => {
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    let shadow = if root == "ring" {
                        format!(
                            "var(--tw-ring-inset,) 0 0 0 calc({width} + var(--tw-ring-offset-width)) var(--tw-ring-color, currentcolor)"
                        )
                    } else {
                        format!("inset 0 0 0 {width} var(--tw-inset-ring-color, currentcolor)")
                    };
                    return Body::new(&[
                        (&format!("--tw-{root}-shadow"), shadow),
                        ("box-shadow", SHADOW.into()),
                    ]);
                }
                Body::new(&[(&format!("--tw-{root}-color"), self.color(value?, modifier)?)])
            }
            "ring-offset" => {
                let value = value?;
                if let Some(width) = width("0px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        ("--tw-ring-offset-width", width),
                        ("--tw-ring-offset-shadow", "var(--tw-ring-inset,) 0 0 0 var(--tw-ring-offset-width) var(--tw-ring-offset-color)".into()),
                    ]);
                }
                Body::new(&[("--tw-ring-offset-color", self.color(value, modifier)?)])
            }
            "outline" => {
                if let Some(width) = width("1px").filter(|_| modifier.is_none()) {
                    return Body::new(&[
                        ("outline-style", "var(--tw-outline-style)".into()),
                        ("outline-width", width),
                    ]);
                }
                Body::new(&[("outline-color", self.color(value?, modifier)?)])
            }
            "outline-offset" | "underline-offset" => {
                let resolved = match (&raw, bare) {
                    (Some(raw), _) => raw.clone(),
                    (_, Some(value)) if integer(value) => format!("{value}px"),
                    _ => return None,
                };
                let property = if root == "outline-offset" {
                    "outline-offset"
                } else {
                    "text-underline-offset"
                };
                Body::new(&[(property, negate(resolved))])
            }
            "decoration" => {
                let value = value?;
                let thickness = match (hint, &raw, bare) {
                    (Some("length" | "percentage"), Some(raw), _) => Some(raw.clone()),
                    (None, Some(raw), _) if is_length(raw) => Some(raw.clone()),
                    (_, None, Some(value)) if integer(value) => Some(format!("{value}px")),
                    _ => None,
                };
                if let Some(thickness) = thickness {
                    return Body::new(&[("text-decoration-thickness", thickness)]);
                }
                Body::new(&[("text-decoration-color", self.color(value, modifier)?)])
            }
            "stroke" => {
                let value = value?;
                let width = match (hint, &raw, bare) {
                    (Some("length" | "number" | "percentage"), Some(raw), _) => Some(raw.clone()),
                    (None, Some(raw), _) if is_length(raw) => Some(raw.clone()),
                    (_, None, Some(value)) if integer(value) => Some(value.to_owned()),
                    _ => None,
                };
                if let Some(width) = width {
                    return Body::new(&[("stroke-width", width)]);
                }
                Body::new(&[("stroke", self.color(value, modifier)?)])
            }
            "fill" | "accent" | "caret" | "bg-glass" => {
                let property = match root {
                    "fill" => "fill",
                    "accent" => "accent-color",
                    "bg-glass" => "-macos-glass-tint-color",
                    _ => "caret-color",
                };
                let mut body = Body::new(&[(property, self.color(value?, modifier)?)])?;
                if root == "bg-glass" && cfg!(target_os = "macos") {
                    body = body
                        .nest(
                            &[],
                            ":where(&)",
                            "appearance:none;background-color:transparent;border:0",
                        )
                        .nest(
                            &[],
                            ":where(&:not(.bg-glass-clear))",
                            "-macos-glass:regular",
                        );
                    body.sort = Some("-macos-glass-tint-color");
                }
                Some(body)
            }
            "placeholder" => {
                let color = self.color(value?, modifier)?;
                Some(Body::default().nest(&[], "&::placeholder", &format!("color:{color}")))
            }
            "scrollbar-thumb" | "scrollbar-track" => {
                let color = self.color(value?, modifier)?;
                Body::new(&[
                    (&format!("--tw-{root}"), color),
                    (
                        "scrollbar-color",
                        "var(--tw-scrollbar-thumb) var(--tw-scrollbar-track)".into(),
                    ),
                ])
            }
            "duration" | "delay" | "ease" => {
                if bare == Some("initial") && root != "delay" {
                    return Body::new(&[(&format!("--tw-{root}"), "initial".into())]);
                }
                let resolved = match (&raw, bare) {
                    (Some(raw), _) => raw.clone(),
                    (_, Some("linear")) if root == "ease" => "linear".into(),
                    (_, Some(value)) if root == "ease" => self.theme("--ease", value)?,
                    (_, Some(value)) if integer(value) => format!("{value}ms"),
                    _ => return None,
                };
                match root {
                    "delay" => Body::new(&[("transition-delay", resolved)]),
                    "duration" => Body::new(&[
                        ("--tw-duration", resolved.clone()),
                        ("transition-duration", resolved),
                    ]),
                    _ => Body::new(&[
                        ("--tw-ease", resolved.clone()),
                        ("transition-timing-function", resolved),
                    ]),
                }
            }
            "transition" => {
                let properties = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (None, None) => format!(
                        "{COLORS}, opacity, box-shadow, transform, translate, scale, rotate, filter, -webkit-backdrop-filter, backdrop-filter, display, content-visibility, overlay, pointer-events"
                    ),
                    (Some("all"), _) => "all".into(),
                    (Some("colors"), _) => COLORS.into(),
                    (Some("opacity"), _) => "opacity".into(),
                    (Some("shadow"), _) => "box-shadow".into(),
                    (Some("transform"), _) => "transform, translate, scale, rotate".into(),
                    (Some("none"), _) => {
                        return Body::new(&[("transition-property", "none".into())]);
                    }
                    _ => return None,
                };
                let mut body = Body::parse(TRANSITION);
                body.declarations
                    .insert(0, ("transition-property".into(), properties));
                Some(body)
            }
            "blur" | "backdrop-blur" => {
                let resolved = match (value, bare, &raw) {
                    (None, ..) => "blur(8px)".into(),
                    (_, Some("none"), _) => " ".into(),
                    (_, _, Some(raw)) => format!("blur({raw})"),
                    (_, Some(value), _) => format!("blur({})", self.theme("--blur", value)?),
                    _ => return None,
                };
                if root == "blur" {
                    return Body::new(&[("--tw-blur", resolved), ("filter", FILTER.into())]);
                }
                Body::new(&[
                    ("--tw-backdrop-blur", resolved),
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ])
            }
            "brightness"
            | "contrast"
            | "saturate"
            | "backdrop-brightness"
            | "backdrop-contrast"
            | "backdrop-saturate"
            | "backdrop-opacity" => filter(root.trim_start_matches("backdrop-"), "%", None),
            "grayscale" | "invert" | "sepia" | "backdrop-grayscale" | "backdrop-invert"
            | "backdrop-sepia" => filter(root.trim_start_matches("backdrop-"), "%", Some("100%")),
            "hue-rotate" | "backdrop-hue-rotate" => filter("hue-rotate", "deg", None),
            "filter" | "backdrop-filter" if value.is_none() => {
                if root == "filter" {
                    return Body::new(&[("filter", FILTER.into())]);
                }
                Body::new(&[
                    ("-webkit-backdrop-filter", BACKDROP.into()),
                    ("backdrop-filter", BACKDROP.into()),
                ])
            }
            "filter" | "backdrop-filter" => {
                let raw = raw?;
                if root == "filter" {
                    return Body::new(&[("filter", raw)]);
                }
                Body::new(&[
                    ("-webkit-backdrop-filter", raw.clone()),
                    ("backdrop-filter", raw),
                ])
            }
            "animate" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => "none".into(),
                    (Some(value), _) => self.theme("--animate", value)?,
                    _ => return None,
                };
                Body::new(&[("animation", resolved)])
            }
            "content" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("none"), _) => {
                        return Body::new(&[
                            ("--tw-content", "none".into()),
                            ("content", "none".into()),
                        ]);
                    }
                    _ => return None,
                };
                Body::new(&[
                    ("--tw-content", resolved),
                    ("content", "var(--tw-content)".into()),
                ])
            }
            "aspect" => {
                let resolved = match (bare, &raw) {
                    (_, Some(raw)) => raw.clone(),
                    (Some("auto"), _) => "auto".into(),
                    (Some("square"), _) => "1 / 1".into(),
                    (Some(value), _) if integer(value) && modifier.is_some_and(integer) => {
                        format!("{value} / {}", modifier?)
                    }
                    (Some(value), _) if modifier.is_none() => self.theme("--aspect", value)?,
                    _ => return None,
                };
                Body::new(&[("aspect-ratio", resolved)])
            }
            "cursor" | "will-change" | "contain" | "list" | "list-image" | "align" | "object"
            | "font-features" | "mask-radial-at" => {
                let property = match root {
                    "font-features" => "font-feature-settings",
                    "mask-radial-at" => "--tw-mask-radial-position",
                    "list" => "list-style-type",
                    "list-image" => "list-style-image",
                    "align" => "vertical-align",
                    "object" => "object-position",
                    _ => root,
                };
                Body::new(&[(property, raw?)])
            }
            "@container" => {
                let kind = match bare {
                    None => "inline-size",
                    Some(kind @ ("normal" | "size")) => kind,
                    _ => return None,
                };
                let mut body = vec![("container-type", kind.to_owned())];
                if let Some(name) = modifier {
                    body.push((
                        "container-name",
                        named_value(name).then(|| name.to_owned())?,
                    ));
                }
                Body::new(&body)
            }
            "outline-hidden" if value.is_none() => Some(
                Body::parse("--tw-outline-style:none;outline-style:none").nest(
                    &["@media (forced-colors: active)"],
                    "&",
                    "outline:2px solid transparent;outline-offset:2px",
                ),
            ),
            "container" if value.is_none() => {
                let mut breakpoints: Vec<_> = DEFAULTS
                    .keys()
                    .filter_map(|name| name.strip_prefix("--breakpoint-"))
                    .map(str::to_owned)
                    .collect();
                breakpoints.extend(
                    self.theme
                        .keys()
                        .filter_map(|name| name.strip_prefix("--breakpoint-"))
                        .map(str::to_owned),
                );
                let mut widths: Vec<_> = breakpoints
                    .into_iter()
                    .filter_map(|name| self.raw(&format!("--breakpoint-{name}")))
                    .collect();
                widths.sort_by_cached_key(|width| {
                    self.media(">=", width, "min").map(|((_, key), _)| key)
                });
                widths.dedup();
                let mut body = Body::new(&[("width", "100%".into())])?;
                for width in widths {
                    body = body.nest(
                        &[&format!("@media (width >= {width})")],
                        "&",
                        &format!("max-width:{width}"),
                    );
                }
                Some(body)
            }
            _ => None,
        }
    }
}
