use super::*;
use crate::style::tailwind::tables::{
    BACKDROP, Bare, EDGES, FILTER, MASK, ROOT, SHADOW, Spec, TEXT, TRANSFORM, Typed, find,
};
use crate::style::tailwind::value::{
    arbitrary, call, calls, decode, direction, infer, integer, math, named_value, nest, quarter,
    shadow_color, split, statements, valid, with_alpha, zero,
};

pub(super) type Branch = (Vec<String>, Vec<String>);

// The parts of a class that a custom utility reads, and what its declarations used.
#[derive(Default)]
struct Scope<'a> {
    value: Option<&'a str>,
    modifier: Option<&'a str>,
    valued: bool,
    modified: bool,
    ratio: bool,
    only: bool,
}

impl Config {
    pub(super) fn compile(&self, class: &str, base: &str, important: bool) -> Option<Compiled> {
        if self.excluded.contains(class) {
            return None;
        }
        let mut parts = split(class, ':');
        if parts.len() > 16 {
            return None;
        }
        if !self.prefix.is_empty() && (parts.len() == 1 || parts.remove(0) != self.prefix) {
            return None;
        }
        let utility = parts.pop()?;
        let (utility, important) = match utility
            .strip_suffix('!')
            .or_else(|| utility.strip_prefix('!'))
        {
            Some(utility) => (utility, true),
            None => (utility, important),
        };
        let body = self.utility(utility)?;
        let (branches, mut ranks, content) = self.branches(&parts, base, 0)?;
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
        let pairs = |pairs: &Pairs| -> Option<Pairs> {
            pairs
                .iter()
                .map(|(key, value)| {
                    let value = match calls(value) {
                        true => self.resolved(value)? + suffix,
                        false if suffix.is_empty() => value.clone(),
                        false => format!("{value}{suffix}"),
                    };
                    Some((key.clone(), value))
                })
                .collect()
        };
        let mut rules = Vec::new();
        let content_pair = || ("content".to_owned(), "var(--tw-content)".to_owned());
        let joined = content == branches && !body.declarations.is_empty();
        if !joined {
            for (at, selectors) in &content {
                rules.push((at.clone(), selectors.join(","), vec![content_pair()]));
            }
        }
        for (at, selectors) in &branches {
            if !body.declarations.is_empty() {
                let mut declarations = pairs(&body.declarations)?;
                if joined && !declarations.iter().any(|(key, _)| key == "content") {
                    declarations.insert(0, content_pair());
                }
                rules.push((at.clone(), selectors.join(","), declarations));
            }
            for (nested, template, values) in &body.nested {
                let mut at = at.clone();
                at.extend(nested.iter().cloned());
                let list: Vec<_> = selectors
                    .iter()
                    .map(|selector| nest(template, selector))
                    .collect();
                rules.push((at, list.join(","), pairs(values)?));
            }
        }
        Some(Compiled { sort, rules })
    }

    // The at-rules and selectors of `base` behind a chain of variants, their ranks, and the pseudo-elements
    // that need `content`.
    pub(super) fn branches(
        &self,
        names: &[&str],
        base: &str,
        depth: u8,
    ) -> Option<(Vec<Branch>, Vec<Rank>, Vec<Branch>)> {
        let mut branches = vec![(Vec::<String>::new(), vec![base.to_owned()])];
        let mut ranks = Vec::new();
        let mut content = Vec::new();
        // Cap selector size: comma parts and & expansion multiply at each variant.
        let (mut size, limit) = (0, (base.len() + 64) * 256);
        for name in names {
            let (rank, variant) = self.variant(name, depth)?;
            if variant.relative {
                return None;
            }
            ranks.push(rank);
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
                                    let selector = nest(part, selector);
                                    self.spend(selector.len())?;
                                    list.push(selector);
                                }
                            }
                            list
                        }
                    };
                    self.spend(0)?;
                    next.push((at, selectors));
                }
            }
            if next.len() > 16 {
                return None;
            }
            if variant.content {
                content.extend(next.iter().cloned());
            }
            branches = next;
        }
        Some((branches, ranks, content))
    }

    // The at-rules and selectors of `@variant` with a list of variants, like `hover, dark:focus`.
    pub(super) fn variant_list(&self, list: &str, base: &str, depth: u8) -> Option<Vec<Branch>> {
        let mut all = Vec::new();
        for variants in split(list, ',') {
            let names: Vec<_> = split(variants, ':').into_iter().map(str::trim).collect();
            all.extend(self.branches(&names, base, depth)?.0);
        }
        (all.len() <= 64).then_some(all)
    }

    // The bodies of the custom utilities for a class, the first one first.
    pub(super) fn custom(&self, class: &str) -> Vec<Body> {
        if self.utilities.is_empty() {
            return Vec::new();
        }
        let (candidate, modifier) = match split(class, '/')[..] {
            [candidate] => (candidate, None),
            [candidate, modifier] if !modifier.is_empty() => (candidate, Some(modifier)),
            _ => (class, None),
        };
        self.utilities
            .iter()
            .enumerate()
            .filter_map(|(index, (name, body))| {
                let value = match name.strip_suffix("-*") {
                    Some(prefix) => match candidate.strip_prefix(prefix)? {
                        "" => Some(""),
                        rest => Some(rest.strip_prefix('-').filter(|value| !value.is_empty())?),
                    },
                    None if name == class => None,
                    None => return None,
                };
                let modifier = modifier.filter(|_| value.is_some());
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
                self.spend(0)?;
                let outer = self.cut.replace(false);
                self.expanding.borrow_mut().push(index);
                let mut scope = Scope {
                    value,
                    modifier,
                    ..Scope::default()
                };
                let body = self.expand(body, &mut scope, 0).filter(|_| {
                    let Scope {
                        valued,
                        ratio,
                        modified,
                        ..
                    } = scope;
                    value.is_none()
                        || valued
                            && !(ratio && modified)
                            && (modifier.is_none() || ratio || modified)
                });
                // A ratio stands for the value and the modifier, so declarations of only one of them go.
                let body = match body {
                    Some(_) if scope.ratio && !scope.only => {
                        scope.only = true;
                        self.expand(&self.utilities[index].1, &mut scope, 0)
                    }
                    body => body,
                };
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
            .collect()
    }

    fn expand(&self, source: &str, scope: &mut Scope, depth: u8) -> Option<Body> {
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
                        .filter_map(|class| self.compile(class, "&", false))
                        .collect();
                    compiled.sort_unstable_by(|a, b| a.sort.cmp(&b.sort));
                    for (at, selector, pairs) in compiled.into_iter().flat_map(|c| c.rules) {
                        self.spend(weigh(&pairs) + selector.len())?;
                        size += weigh(&pairs) + selector.len();
                        body.push(at, selector, pairs);
                    }
                    continue;
                }
                let Some((property, raw)) = prelude.split_once(':') else {
                    continue;
                };
                let property = property.trim().to_owned();
                if property == "--tw-sort" {
                    body.sort = ORDER_INDEX.get_key_value(raw.trim()).map(|(&name, _)| name);
                    continue;
                }
                let mut raw = raw.trim().to_owned();
                let (mut valid, mut ratio, mut valued) = (true, false, false);
                // Never scan substituted text again: it comes from the class and could hold more calls.
                let mut from = 0;
                while let Some(((start, open, close), function, input)) =
                    [("--value", scope.value), ("--modifier", scope.modifier)]
                        .into_iter()
                        .filter_map(|(function, input)| {
                            Some((call(&raw[from..], function)?, function, input))
                        })
                        .min_by_key(|((start, ..), ..)| *start)
                {
                    let (start, open, close) = (from + start, from + open, from + close);
                    let modifier = function == "--modifier";
                    let input = input.filter(|value| !value.is_empty());
                    let fraction = scope.modifier.filter(|_| !modifier);
                    let result = (close < raw.len())
                        .then(|| self.resolve(&raw[open..close], input, fraction))
                        .flatten();
                    let Some((result, fraction)) = result else {
                        valid = false;
                        break;
                    };
                    raw.replace_range(start..=close, &result);
                    from = start + result.len();
                    ratio |= fraction;
                    valued |= !modifier;
                    if modifier {
                        scope.modified = true;
                    } else {
                        scope.valued = true;
                        scope.ratio |= fraction;
                    }
                }
                if valid && !(scope.only && valued && !ratio) {
                    body.push(
                        Vec::new(),
                        "&".into(),
                        vec![(property, self.functions(&raw))],
                    );
                }
                continue;
            };
            let inner = self.expand(inner, scope, depth + 1)?;
            let branches = if let Some(list) = prelude.strip_prefix("@variant ") {
                self.variant_list(list, "&", 0)?
            } else if prelude.starts_with('@') {
                vec![(vec![prelude.to_owned()], vec!["&".to_owned()])]
            } else if prelude.contains('&') {
                vec![(Vec::new(), vec![prelude.to_owned()])]
            } else {
                let list = split(prelude, ',').into_iter();
                let list = list.map(|part| format!("& {}", part.trim())).collect();
                vec![(Vec::new(), list)]
            };
            for (at, selectors) in branches {
                let selector = selectors.join(",");
                if !inner.declarations.is_empty() {
                    size += weigh(&inner.declarations) + selector.len();
                    body.push(at.clone(), selector.clone(), inner.declarations.clone());
                }
                for (nested, template, pairs) in &inner.nested {
                    size += weigh(pairs)
                        + template.len()
                        + template.matches('&').count() * selector.len();
                    if size > 1 << 16 {
                        return None;
                    }
                    self.spend(weigh(pairs) + template.len() + selector.len())?;
                    let mut rules = at.clone();
                    rules.extend(nested.iter().cloned());
                    let selector = match selectors.len() {
                        1 => selector.clone(),
                        _ => format!(":is({selector})"),
                    };
                    body.push(rules, nest(template, &selector), pairs.clone());
                }
            }
        }
        (size <= 1 << 16).then_some(body)
    }

    pub(super) fn utility(&self, class: &str) -> Option<Body> {
        let builtin = self.builtin(class);
        // Like Tailwind, a custom utility with the name of a built-in one adds to it, in sort order.
        let mut bodies = self.custom(class);
        if bodies.is_empty() {
            return builtin;
        }
        bodies.splice(0..0, builtin);
        bodies.sort_by_cached_key(Body::order);
        bodies.into_iter().reduce(Body::merge)
    }

    pub(super) fn builtin(&self, class: &str) -> Option<Body> {
        let (candidate, modifier) = match split(class, '/')[..] {
            [candidate] => (candidate, None),
            [candidate, modifier] => (candidate, Some(Modifier::parse(modifier)?)),
            _ => return None,
        };
        if let Some(raw) = candidate
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
            let value = decode(value);
            return (valid(&value) && !value.trim().is_empty())
                .then(|| self.alpha(value, modifier.as_ref()))
                .flatten()
                .and_then(|value| Body::new(&[(property, value)]));
        }
        let fixed = find(TEXT.statics, &TEXT.static_lines, class).map(|css| Body::fill(css, ""));
        // The theme of the page can replace the words of a utility, like `rounded-full`.
        if fixed.is_some() && self.theme.is_empty() {
            return fixed;
        }
        let (negative, candidate) = match candidate.strip_prefix('-') {
            Some(candidate) => (true, candidate),
            None => (false, candidate),
        };
        let functional = |root: &str, value: Option<&str>| {
            self.functional(root, value, modifier.as_ref(), negative)
        };
        if matches!(
            candidate,
            "bg-glass" | "bg-glass-clear" | "bg-glass-interactive"
        ) {
            return None;
        }
        let body = if let Some(value) = candidate.strip_prefix("bg-glass-") {
            functional("bg-glass", Some(value))
        } else if candidate.ends_with([']', ')']) {
            let open = if candidate.ends_with(']') { "-[" } else { "-(" };
            let index = candidate.find(open)?;
            functional(&candidate[..index], Some(&candidate[index + 1..]))
        } else {
            // Scan valid suffix boundaries once, and only as far as the longest root: long names are from pages.
            let named = candidate
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '%' | '-')))
                .map_or(0, |index| index + 1);
            let mut end = candidate.len().min(ROOT + 1);
            while !candidate.is_char_boundary(end) {
                end -= 1;
            }
            let whole = match candidate.len() <= ROOT {
                true => functional(candidate, None),
                false => None,
            };
            whole.or_else(|| {
                candidate[..end]
                    .match_indices('-')
                    .rev()
                    .take_while(|(index, _)| *index >= named)
                    .filter(|(index, _)| *index > 0 && index + 1 < candidate.len())
                    .find_map(|(index, _)| {
                        functional(&candidate[..index], Some(&candidate[index + 1..]))
                    })
            })
        };
        match (fixed, body) {
            (Some(fixed), Some(body)) => Some(fixed.merge(body)),
            (fixed, body) => fixed.or(body),
        }
    }

    pub(super) fn functional(
        &self,
        root: &str,
        value: Option<&str>,
        modifier: Option<&Modifier>,
        negative: bool,
    ) -> Option<Body> {
        let known = |names: &[&str]| names.contains(&root);
        let (spec, typed) = match find(TEXT.specs, &TEXT.spec_lines, root) {
            Some(line) => match Spec::parse(line) {
                Some(spec) => (Some(spec), None),
                None => (None, Typed::parse(line)),
            },
            None => (None, None),
        };
        let mut side = None;
        if spec.is_none() && typed.is_none() {
            side = root.strip_prefix("mask-").and_then(|rest| {
                let (edge, end) = rest.rsplit_once('-')?;
                let edges = ["x", "y", "t", "r", "b", "l", "linear", "radial", "conic"];
                (matches!(end, "from" | "to") && edges.contains(&edge)).then_some((edge, end))
            });
            if side.is_none()
                && !matches!(
                    root,
                    "@container"
                        | "backdrop-filter"
                        | "bg"
                        | "bg-conic"
                        | "bg-linear"
                        | "bg-radial"
                        | "container"
                        | "drop-shadow"
                        | "filter"
                        | "flex"
                        | "font"
                        | "inset-shadow"
                        | "mask"
                        | "max-w-screen"
                        | "outline-hidden"
                        | "rotate"
                        | "scale"
                        | "shadow"
                        | "text"
                        | "text-shadow"
                        | "transform"
                )
            {
                return None;
            }
        }
        if negative
            && !spec.as_ref().is_some_and(|spec| spec.negative)
            && !known(&["bg-conic", "bg-linear", "rotate", "scale"])
        {
            return None;
        }
        let (bare, raw) = match value {
            Some(value) if value.starts_with(['[', '(']) => (None, Some(self.typed(value)?)),
            _ => (value, None),
        };
        let hint = raw.as_ref().and_then(|(hint, _)| hint.as_deref());
        let raw = raw.as_ref().map(|(_, raw)| raw.as_str());
        let kind = |types: &'static str| hint.or_else(|| infer(raw?, types));
        let alpha = |color: &str| self.alpha(color.to_owned(), modifier);
        let color = |keys: &str| {
            let color = match bare? {
                "inherit" => "inherit".into(),
                "transparent" => "transparent".into(),
                "current" => "currentcolor".into(),
                bare => self.themed(keys, bare)?,
            };
            self.alpha(color, modifier)
        };
        let negate = |value: String| match negative {
            true => math(&format!("calc({value} * -1)")),
            false => value,
        };
        // The numbers of a fraction like `1/2`.
        let fraction = match (bare, modifier) {
            (Some(bare), Some(Modifier::Named(modifier))) => Some((bare, *modifier)),
            _ => None,
        };
        if let Some(spec) = spec {
            if let Bare::Color = spec.bare {
                let color = match raw {
                    Some(raw)
                        if root == "bg-glass"
                            && (kind("color") != Some("color") && !raw.starts_with("var(")
                                || hint.is_some_and(|hint| hint != "color")) =>
                    {
                        return None;
                    }
                    Some(raw) => alpha(raw)?,
                    None => color(spec.keys)?,
                };
                let mut body = Body::fill(spec.css, &color);
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
                return Some(body);
            }
            let resolved = match (bare, raw) {
                (_, Some(_)) if modifier.is_some() => return None,
                (_, Some(raw)) => negate(raw.to_owned()),
                (None, None) => {
                    if modifier.is_some()
                        || matches!(spec.bare, Bare::Spacing | Bare::Start | Bare::Gap)
                    {
                        return None;
                    }
                    negate(match spec.word("") {
                        Some(value) if value.starts_with("--") => {
                            self.raw(value).unwrap_or("1px").to_owned()
                        }
                        Some(value) => value.to_owned(),
                        None => spec.keys.split(' ').find_map(|key| self.variable(key))?,
                    })
                }
                (Some(bare), None) => {
                    let themed = match fraction {
                        Some((value, modifier)) => {
                            self.themed(spec.keys, &format!("{value}/{modifier}"))
                        }
                        None => self.themed(spec.keys, bare),
                    };
                    let resolved = match (themed, fraction) {
                        (Some(_), None) if modifier.is_some() => return None,
                        (Some(themed), _) => Some(negate(themed)),
                        (None, Some((value, modifier))) => {
                            let ratio = matches!(spec.bare, Bare::Ratio);
                            if ratio && quarter(value) && quarter(modifier) {
                                Some(format!("{value}/{modifier}"))
                            } else if spec.fraction && integer(value) && integer(modifier) {
                                Some(negate(format!("calc({value} / {modifier} * 100%)")))
                            } else {
                                return None;
                            }
                        }
                        (None, None) if modifier.is_some() => return None,
                        (None, None) => self.bare(&spec, bare, negative),
                    };
                    match resolved {
                        Some(resolved) => resolved,
                        None => {
                            let word = spec.word(bare).filter(|_| !negative || bare == "full")?;
                            let sign = if negative { "-" } else { "" };
                            format!("{sign}{word}")
                        }
                    }
                }
            };
            if matches!(spec.bare, Bare::Gap) && zero(&resolved) {
                let css = spec
                    .css
                    .split(';')
                    .map(|item| match item.split_once(":calc(") {
                        Some((name, _)) => format!("{name}:0"),
                        None => item.to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join(";");
                return Some(Body::fill(&css, ""));
            }
            let mut body = Body::fill(spec.css, &resolved);
            if root == "transition" {
                for (index, name, default) in
                    [(1, "timing-function", "ease"), (2, "duration", "0s")]
                {
                    let name = format!("--default-transition-{name}");
                    let value = self.variable(&name).unwrap_or_else(|| default.into());
                    let pair = &mut body.declarations[index].1;
                    *pair = pair.replace(&format!("var({name})"), &value);
                }
            }
            return Some(body);
        }
        if let Some(typed) = typed {
            if let Some(raw) = raw {
                let wide = kind(typed.types).is_some_and(|kind| {
                    kind != "color" && typed.types.split(' ').any(|name| name == kind)
                });
                return match (wide, modifier) {
                    (true, Some(_)) => None,
                    (true, None) => Some(typed.wide(self, root, raw)),
                    _ => Some(Body::fill(typed.color, &alpha(raw)?)),
                };
            }
            let Some(bare) = bare else {
                if typed.default.is_empty() || modifier.is_some() {
                    return None;
                }
                let width = match typed.default.starts_with("--") {
                    true => self.raw(typed.default).unwrap_or("1px").to_owned(),
                    false => typed.default.into(),
                };
                return Some(typed.wide(self, root, &width));
            };
            let width = || {
                self.themed(typed.widths, bare).or_else(|| {
                    let number = match typed.unit {
                        "%" => bare.strip_suffix('%')?,
                        _ => bare,
                    };
                    integer(number).then(|| format!("{number}{}", typed.unit))
                })
            };
            let first = known(&["decoration", "ring-offset"]);
            if first && let Some(width) = width() {
                return modifier.is_none().then(|| typed.wide(self, root, &width));
            }
            if let Some(color) = color(typed.colors) {
                return Some(Body::fill(typed.color, &color));
            }
            let width = width().filter(|_| !first && modifier.is_none())?;
            return Some(typed.wide(self, root, &width));
        }
        if let Some((edge, end)) = side {
            let (field, assigned) = match (bare, raw) {
                (_, Some(raw)) => match kind("length percentage color") {
                    Some("color") => ("color", alpha(raw)?),
                    kind => {
                        let whole = || integer(raw.strip_suffix('%').unwrap_or_default());
                        (modifier.is_none() && (kind != Some("percentage") || whole()))
                            .then(|| ("position", raw.to_owned()))?
                    }
                },
                (Some(bare), _) => match color("--background-color --color") {
                    Some(color) => ("color", color),
                    None if modifier.is_some() => return None,
                    None => match bare.strip_suffix('%') {
                        Some(number) if integer(number) => ("position", bare.to_owned()),
                        Some(_) => return None,
                        None if quarter(bare) => ("position", self.function("--spacing", bare)?),
                        None => return None,
                    },
                },
                _ => return None,
            };
            let mut body = Body::fill(MASK, "");
            let mut push = |name: String, value: String| body.declarations.push((name, value));
            if let "linear" | "radial" | "conic" = edge {
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
                return Some(body);
            }
            let sides: &[&str] = match edge {
                "x" => &["right", "left"],
                "y" => &["top", "bottom"],
                "t" => &["top"],
                "r" => &["right"],
                "b" => &["bottom"],
                _ => &["left"],
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
            return Some(body);
        }
        // Only these utilities take a modifier.
        if modifier.is_some()
            && !known(&[
                "@container",
                "bg",
                "bg-conic",
                "bg-linear",
                "bg-radial",
                "drop-shadow",
                "flex",
                "inset-shadow",
                "max-w-screen",
                "shadow",
                "text",
                "text-shadow",
            ])
        {
            return None;
        }
        let plain = modifier.is_none();
        match root {
            "max-w-screen" => Body::new(&[("max-width", self.themed("--breakpoint", bare?)?)]),
            "flex" => {
                let resolved = match (bare, raw, fraction) {
                    (_, Some(raw), _) if plain => raw.to_owned(),
                    (_, _, Some((value, modifier))) if integer(value) && integer(modifier) => {
                        format!("calc({value}/{modifier} * 100%)")
                    }
                    (Some(value), _, _) if plain && integer(value) => value.to_owned(),
                    _ => return None,
                };
                Body::new(&[("flex", resolved)])
            }
            "rotate" => {
                let resolved = match (bare, raw) {
                    (_, Some(raw)) if kind("angle vector") == Some("vector") => {
                        return Body::new(&[("rotate", format!("{raw} var(--tw-rotate)"))]);
                    }
                    (_, Some(raw)) => raw.to_owned(),
                    (Some(bare), _) => self
                        .themed("--rotate", bare)
                        .or_else(|| integer(bare).then(|| format!("{bare}deg")))?,
                    _ => return None,
                };
                Body::new(&[("rotate", negate(resolved))])
            }
            "scale" => {
                if let Some(raw) = raw {
                    return Body::new(&[("scale", negate(raw.to_owned()))]);
                }
                let resolved = negate(
                    self.themed("--scale", bare?)
                        .or_else(|| integer(bare?).then(|| format!("{}%", bare.unwrap_or("0"))))?,
                );
                Some(Body::fill(
                    "--tw-scale-x:$;--tw-scale-y:$;--tw-scale-z:$;scale:var(--tw-scale-x) var(--tw-scale-y)",
                    &resolved,
                ))
            }
            "transform" => match (value, raw) {
                (None, _) => Body::new(&[("transform", TRANSFORM.into())]),
                (_, Some(raw)) => Body::new(&[("transform", raw.to_owned())]),
                _ => None,
            },
            "filter" | "backdrop-filter" => {
                let css = match root {
                    "filter" => "filter:$",
                    _ => "-webkit-backdrop-filter:$;backdrop-filter:$",
                };
                match (bare, raw) {
                    (None, None) if root == "filter" => Some(Body::fill(css, FILTER)),
                    (None, None) => Some(Body::fill(BACKDROP, "")),
                    (_, Some(raw)) => Some(Body::fill(css, raw)),
                    (Some("none"), _) => Some(Body::fill(css, "none")),
                    _ => None,
                }
            }
            "bg" => {
                if let Some(raw) = raw {
                    let property = match kind("image color percentage position bg-size length url")
                    {
                        Some("percentage" | "position") => "background-position",
                        Some("bg-size" | "length" | "size") => "background-size",
                        Some("image" | "url") => "background-image",
                        _ => return Body::new(&[("background-color", alpha(raw)?)]),
                    };
                    return Body::new(&[(property, raw.to_owned())]).filter(|_| plain);
                }
                if let Some(color) = color("--background-color --color") {
                    return Body::new(&[("background-color", color)]);
                }
                let image = self.themed("--background-image", bare?)?;
                Body::new(&[("background-image", image)]).filter(|_| plain)
            }
            "bg-linear" | "bg-conic" | "bg-radial" => {
                let kind_name = &root[3..];
                let image = |stops: &str| format!("{kind_name}-gradient({stops})");
                if let Some(raw) = raw {
                    if !plain {
                        return None;
                    }
                    let raw = match root {
                        "bg-linear" if kind("angle") == Some("angle") => negate(raw.to_owned()),
                        "bg-linear" if negative => return None,
                        _ => raw.to_owned(),
                    };
                    return Body::new(&[
                        ("--tw-gradient-position", raw.clone()),
                        (
                            "background-image",
                            image(&format!("var(--tw-gradient-stops,{raw})")),
                        ),
                    ]);
                }
                let interpolation = match modifier {
                    None => "in oklab".into(),
                    Some(Modifier::Raw(raw)) => raw.clone(),
                    Some(Modifier::Named(
                        name @ ("longer" | "shorter" | "increasing" | "decreasing"),
                    )) => format!("in oklch {name} hue"),
                    Some(Modifier::Named(name)) => format!("in {name}"),
                };
                let angle = |bare: &str| {
                    integer(bare).then(|| match negative {
                        true => format!("calc({bare}deg * -1)"),
                        false => format!("{bare}deg"),
                    })
                };
                let image = ("background-image", image("var(--tw-gradient-stops)"));
                let position = match (root, bare) {
                    ("bg-linear", Some(bare)) => match bare.strip_prefix("to-") {
                        Some(side) if !negative => format!("to {}", direction(side)?),
                        _ => angle(bare)?,
                    },
                    ("bg-conic", Some(bare)) => format!("from {} {interpolation}", angle(bare)?),
                    ("bg-conic" | "bg-radial", None) => interpolation.clone(),
                    _ => return None,
                };
                if root != "bg-linear" {
                    return Body::new(&[("--tw-gradient-position", position), image]);
                }
                let mut body = Body::new(&[("--tw-gradient-position", position.clone())])?;
                let nested = |at: &[&str], name: &str, value: String| {
                    let at = at.iter().map(|&rule| rule.to_owned()).collect();
                    (at, "&".to_owned(), vec![(name.to_owned(), value)])
                };
                body.nested = vec![
                    nested(
                        &["@supports (background-image: linear-gradient(in lab, red, red))"],
                        "--tw-gradient-position",
                        format!("{position} {interpolation}"),
                    ),
                    nested(&[], image.0, image.1),
                ];
                Some(body)
            }
            "mask" => {
                let property = match kind("image percentage position bg-size length url") {
                    Some("percentage" | "position") => "mask-position",
                    Some("bg-size" | "length" | "size") => "mask-size",
                    _ => "mask-image",
                };
                Body::new(&[(property, raw?.to_owned())])
            }
            "text" => {
                let leading = || match modifier? {
                    Modifier::Raw(raw) => Some(raw.clone()),
                    Modifier::Named(name) => self
                        .themed("--leading", name)
                        .or_else(|| quarter(name).then(|| self.function("--spacing", name))?)
                        .or_else(|| (*name == "none").then(|| "1".into())),
                };
                let mut body = if let Some(raw) = raw {
                    match kind("color length percentage absolute-size relative-size") {
                        Some(
                            "size" | "length" | "percentage" | "absolute-size" | "relative-size",
                        ) => vec![("font-size", raw.to_owned())],
                        _ => return Body::new(&[("color", alpha(raw)?)]),
                    }
                } else {
                    if let Some(color) = color("--text-color --color") {
                        return Body::new(&[("color", color)]);
                    }
                    let name = self.key("--text", bare?)?;
                    let mut body = vec![("font-size", self.variable(&name)?)];
                    if plain {
                        for (property, var) in [
                            ("line-height", "leading"),
                            ("letter-spacing", "tracking"),
                            ("font-weight", "font-weight"),
                        ] {
                            if let Some(option) = self.variable(&format!("{name}--{property}")) {
                                body.push((property, format!("var(--tw-{var}, {option})")));
                            }
                        }
                    }
                    body
                };
                if !plain {
                    body.push(("line-height", leading()?));
                }
                Body::new(&body)
            }
            "font" => {
                let weight = if let Some(raw) = raw {
                    match kind("number generic-name family-name") {
                        Some("generic-name" | "family-name") => {
                            return Body::new(&[("font-family", raw.to_owned())]);
                        }
                        _ => raw.to_owned(),
                    }
                } else {
                    if let Some(name) = self.key("--font", bare?) {
                        let mut body = vec![("font-family", self.variable(&name)?)];
                        for property in ["font-feature-settings", "font-variation-settings"] {
                            if let Some(option) = self.variable(&format!("{name}--{property}")) {
                                body.push((property, option));
                            }
                        }
                        return Body::new(&body);
                    }
                    self.themed("--font-weight", bare?)?
                };
                Some(Body::fill("--tw-font-weight:$;font-weight:$", &weight))
            }
            "shadow" | "inset-shadow" | "text-shadow" | "drop-shadow" => {
                let opacity = match modifier {
                    None => None,
                    Some(Modifier::Raw(raw)) => Some(raw.clone()),
                    Some(Modifier::Named(name)) => quarter(name).then(|| format!("{name}%")),
                };
                let tint = |color: String| {
                    let color = match color.as_str() {
                        "inherit" => color,
                        _ => with_alpha(&color, &format!("var(--tw-{root}-alpha)")),
                    };
                    let mut body = vec![(format!("--tw-{root}-color"), color)];
                    if root == "drop-shadow" {
                        body.push((
                            "--tw-drop-shadow".into(),
                            "var(--tw-drop-shadow-size)".into(),
                        ));
                    }
                    Some(Body {
                        declarations: body,
                        ..Body::default()
                    })
                };
                let name = match bare {
                    Some(bare) => format!("--{root}-{bare}"),
                    None => format!("--{root}"),
                };
                let shadow = match (bare, raw) {
                    (_, Some(raw)) if kind("color") == Some("color") => {
                        return tint(alpha(raw)?);
                    }
                    (_, Some(raw)) => raw.to_owned(),
                    (Some("none"), _) if root != "drop-shadow" => match root {
                        _ if !plain => return None,
                        "text-shadow" => return Body::new(&[("text-shadow", "none".into())]),
                        _ => {
                            let inset = if root == "shadow" { "" } else { "inset " };
                            return Body::new(&[
                                (&format!("--tw-{root}"), format!("{inset}0 0 #0000")),
                                ("box-shadow", SHADOW.into()),
                            ]);
                        }
                    },
                    (Some("inherit"), _) if root != "drop-shadow" => {
                        return plain.then(|| tint("inherit".into())).flatten();
                    }
                    _ => match self.raw(&name) {
                        Some(shadow) => shadow.to_owned(),
                        None => {
                            let keys = match root {
                                "text-shadow" => "--text-shadow-color --color",
                                "drop-shadow" => "--drop-shadow-color --color",
                                _ => "--box-shadow-color --color",
                            };
                            return tint(color(keys)?);
                        }
                    },
                };
                if !plain && opacity.is_none() {
                    return None;
                }
                let variable = format!("--tw-{root}-color");
                let colored = |layer: &str| shadow_color(layer, &variable, opacity.as_deref());
                let mut body = Vec::new();
                if let Some(opacity) = &opacity {
                    body.push((format!("--tw-{root}-alpha"), opacity.clone()));
                }
                let layers =
                    |shadow: &str, map: &dyn Fn(&str) -> String, start: &str, end: &str| {
                        split(shadow, ',')
                            .iter()
                            .map(|layer| format!("{start}{}{end}", map(layer.trim())))
                            .collect::<Vec<_>>()
                    };
                match root {
                    "shadow" | "inset-shadow" | "text-shadow" => {
                        let inset = match (root, raw) {
                            ("inset-shadow", Some(_)) => "inset ",
                            _ => "",
                        };
                        let value = layers(&shadow, &colored, inset, "").join(", ");
                        if root == "text-shadow" {
                            body.push(("text-shadow".into(), value));
                        } else {
                            body.push((format!("--tw-{root}"), value));
                            body.push(("box-shadow".into(), SHADOW.into()));
                        }
                    }
                    _ => {
                        let size = layers(&shadow, &colored, "drop-shadow(", ")").join(" ");
                        let plain = match raw {
                            None if bare.is_none() || opacity.is_none() => {
                                let shadow = self.variable(&name)?;
                                layers(&shadow, &|layer| layer.to_owned(), "drop-shadow(", ")")
                                    .join(" ")
                            }
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
            "@container" => {
                let kind = match (bare, raw) {
                    (None, None) => "inline-size",
                    (_, Some(raw)) => raw,
                    (Some(kind @ ("normal" | "size")), _) => kind,
                    _ => return None,
                };
                let mut body = vec![("container-type", kind.to_owned())];
                match modifier {
                    Some(Modifier::Named(name)) => body.push(("container-name", (*name).into())),
                    Some(Modifier::Raw(name)) => body.push(("container-name", name.clone())),
                    None => {}
                }
                Body::new(&body)
            }
            "outline-hidden" if value.is_none() => Some(
                Body::fill("--tw-outline-style:none;outline-style:none", "").nest(
                    &["@media (forced-colors: active)"],
                    "&",
                    "outline:2px solid transparent;outline-offset:2px",
                ),
            ),
            "container" if value.is_none() => {
                let mut widths: Vec<_> = self.namespace("--breakpoint").collect();
                widths.sort_by_cached_key(|width| {
                    self.media(">=", width, "min").map(|((_, key), _)| key)
                });
                let mut body = Body::new(&[("width", "100%".into())])?;
                for width in widths {
                    body = body.nest(
                        &[&format!("@media (width >= {width})")],
                        "&",
                        &format!("max-width:{width}"),
                    );
                }
                body.sort = Some("--tw-container-component");
                Some(body)
            }
            _ => None,
        }
    }

    // The value of a bare number, like `4` in `p-4`.
    fn bare(&self, spec: &Spec, bare: &str, negative: bool) -> Option<String> {
        let sign = if negative { "-" } else { "" };
        match spec.bare {
            Bare::Spacing | Bare::Space | Bare::Gap if quarter(bare) => {
                return self.function("--spacing", &format!("{sign}{bare}"));
            }
            Bare::Spacing | Bare::Start | Bare::Gap if bare == "px" => {
                return Some(format!("{sign}1px"));
            }
            Bare::Start if quarter(bare) => {
                let base = self.variable("--spacing")?;
                return Some(format!("calc({base} * {sign}{bare})"));
            }
            Bare::Angle => {
                return integer(bare).then(|| match bare {
                    "0" => "0deg".into(),
                    "1" => format!("{sign}1deg"),
                    _ => format!("calc(1deg * {sign}{bare})"),
                });
            }
            _ => {}
        }
        let value = match spec.bare {
            Bare::Integer(unit) => integer(bare).then(|| format!("{bare}{unit}")),
            Bare::Opacity => quarter(bare).then(|| format!("{bare}%")),
            Bare::Repeat => {
                (integer(bare) && bare != "0").then(|| format!("repeat({bare}, minmax(0, 1fr))"))
            }
            Bare::Stretch => bare
                .strip_suffix('%')
                .filter(|number| {
                    integer(number)
                        && number
                            .parse()
                            .is_ok_and(|number: u32| (50..=200).contains(&number))
                })
                .map(|_| bare.to_owned()),
            _ => None,
        }?;
        Some(match negative {
            true => format!("calc({value} * -1)"),
            false => value,
        })
    }
}

impl Typed {
    fn wide(&self, config: &Config, root: &str, width: &str) -> Body {
        let mut body = Body::fill(self.width, width);
        if root == "ring"
            && let Some(color) = config.raw("--default-ring-color")
        {
            let shadow = &mut body.declarations[0].1;
            *shadow = shadow.replace("currentcolor", color);
        }
        body
    }
}

pub(super) enum Modifier<'a> {
    Named(&'a str),
    Raw(String),
}

impl<'a> Modifier<'a> {
    pub(super) fn parse(modifier: &'a str) -> Option<Self> {
        if modifier.starts_with(['[', '(']) {
            return arbitrary(modifier).map(Self::Raw);
        }
        named_value(modifier).then_some(Self::Named(modifier))
    }
}
