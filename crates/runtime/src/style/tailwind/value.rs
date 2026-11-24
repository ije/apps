use super::*;
use crate::style::tailwind::tables::{DEFAULTS, INLINE, KEPT};

impl Config {
    pub(super) fn removed(&self, name: &str) -> bool {
        self.cleared.iter().any(|cleared| removes(cleared, name))
    }

    pub(super) fn raw(&self, name: &str) -> Option<String> {
        if let Some(value) = self.theme.get(name) {
            return Some(value.clone());
        }
        if self.removed(name) {
            return None;
        }
        DEFAULTS.get(name).map(|value| (*value).to_owned())
    }

    pub(super) fn theme(&self, namespace: &str, key: &str) -> Option<String> {
        let mut name = format!("{namespace}-{key}");
        let mut value = self.raw(&name);
        if value.is_none() && key.contains('.') {
            name = format!("{namespace}-{}", key.replace('.', "_"));
            value = self.raw(&name);
        }
        if kept(&name, namespace) {
            return None;
        }
        if self.inline.contains(&name) || INLINE.contains(&name.as_str()) {
            return value;
        }
        value.map(|_| format!("var({name})"))
    }

    pub(super) fn resolve(&self, arguments: &str, value: &str) -> Option<String> {
        split(arguments, ',')
            .into_iter()
            .map(str::trim)
            .find_map(|argument| {
                if let Some(namespace) = argument
                    .strip_suffix("-*")
                    .filter(|name| name.starts_with("--"))
                {
                    return named_value(value)
                        .then(|| self.theme(namespace, value))
                        .flatten();
                }
                if let Some(literal) = argument.strip_prefix('"').and_then(|a| a.strip_suffix('"'))
                {
                    return (literal == value).then(|| value.to_owned());
                }
                match (
                    argument.strip_prefix('[').and_then(|a| a.strip_suffix(']')),
                    self.typed(value),
                ) {
                    (Some(kind), Some((hint, raw))) => (kind == "*"
                        || hint.as_deref() == Some(kind)
                        || hint.is_none() && is_type(kind, &raw))
                    .then_some(raw),
                    (None, None) if argument == "ratio" => {
                        is_type(argument, value).then(|| value.replacen('/', " / ", 1))
                    }
                    (None, None) => {
                        (named_value(value) && is_type(argument, value)).then(|| value.to_owned())
                    }
                    _ => None,
                }
            })
    }

    pub(super) fn arbitrary(&self, value: &str) -> Option<String> {
        let raw = match value
            .strip_prefix('[')
            .and_then(|raw| raw.strip_suffix(']'))
        {
            Some(raw) => decode(raw),
            None => {
                let raw = value.strip_prefix('(')?.strip_suffix(')')?;
                let (kind, name) = match split(raw, ':').as_slice() {
                    [kind, name] => (Some(*kind), *name),
                    _ => (None, raw),
                };
                if !name.starts_with("--") || !valid(name) {
                    return None;
                }
                decode(&match kind {
                    Some(kind) => format!("{kind}:var({name})"),
                    None => format!("var({name})"),
                })
            }
        };
        let raw = self.functions(&raw);
        (valid(&raw) && !raw.trim().is_empty()).then_some(raw)
    }

    pub(super) fn typed(&self, value: &str) -> Option<(Option<String>, String)> {
        let raw = self.arbitrary(value)?;
        let end = raw
            .find(|c: char| !(c == '-' || c.is_ascii_lowercase()))
            .unwrap_or(raw.len());
        if raw[end..].starts_with(':') {
            let (hint, rest) = (&raw[..end], &raw[end + 1..]);
            if hint.is_empty() || rest.trim().is_empty() {
                return None;
            }
            return Some((Some(hint.to_owned()), rest.to_owned()));
        }
        Some((None, raw))
    }

    pub(super) fn functions(&self, value: &str) -> String {
        if !calls(value) {
            return value.to_owned();
        }
        // Resolve innermost calls in one pass. Repeated scans of failed calls can be exponential.
        let mut out = String::with_capacity(value.len());
        let mut open: Vec<(usize, Option<&str>)> = Vec::new();
        // Each call reads all of its arguments, so calls nested past this depth stay as written.
        let mut nested = 0;
        let (mut quote, mut chars) = (None, value.chars());
        while let Some(character) = chars.next() {
            out.push(character);
            match character {
                '\\' => out.extend(chars.next()),
                _ if quote.is_some() => {
                    if quote == Some(character) {
                        quote = None;
                    }
                }
                '"' | '\'' => quote = Some(character),
                '(' => {
                    let before = &out[..out.len() - 1];
                    let function = ["--spacing", "--alpha", "--theme", "theme"]
                        .into_iter()
                        .find(|name| {
                            before.strip_suffix(name).is_some_and(|rest| {
                                *name != "theme"
                                    || !rest.ends_with(|c: char| c == '-' || c.is_alphanumeric())
                            })
                        })
                        .filter(|_| nested < 16);
                    nested += usize::from(function.is_some());
                    open.push((before.len() - function.map_or(0, str::len), function));
                }
                ')' => {
                    if let Some((start, Some(function))) = open.pop() {
                        nested -= 1;
                        let arguments = &out[start + function.len() + 1..out.len() - 1];
                        if let Some(result) = self.function(function, arguments.trim()) {
                            out.truncate(start);
                            out.push_str(&result);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub(super) fn function(&self, function: &str, arguments: &str) -> Option<String> {
        match function {
            "--spacing" => {
                let base = self.raw("--spacing")?;
                let base = match self.inline.contains("--spacing") {
                    true => base,
                    false => "var(--spacing)".into(),
                };
                Some(match arguments {
                    "0" => "0px".into(),
                    "1" => base,
                    _ => format!("calc({base} * {arguments})"),
                })
            }
            "--alpha" => {
                let [color, alpha] = split(arguments, '/')[..] else {
                    return None;
                };
                Some(with_alpha(color.trim(), alpha.trim()))
            }
            "--theme" => {
                let (name, fallback) = match split(arguments, ',').as_slice() {
                    [name] => (*name, None),
                    [name, rest @ ..] => (*name, Some(rest.join(",").trim().to_owned())),
                    _ => return None,
                };
                let (name, inline) = match name.trim().strip_suffix(" inline") {
                    Some(name) => (name.trim(), true),
                    None => (name.trim(), false),
                };
                match self.raw(name) {
                    Some(value) if inline || self.inline.contains(name) => Some(value),
                    Some(_) => Some(match fallback {
                        Some(fallback) => format!("var({name}, {fallback})"),
                        None => format!("var({name})"),
                    }),
                    None => fallback,
                }
            }
            _ => {
                // Author CSS separates `spacing` and `.4` with `/**/` when it is serialized.
                let arguments = arguments.replace("/**/", "");
                let (path, alpha) = match split(&arguments, '/').as_slice() {
                    [path] => (path.trim(), None),
                    [path, alpha] => (path.trim(), Some(alpha.trim())),
                    _ => return None,
                };
                let value = if path.starts_with("--") {
                    self.raw(path)?
                } else {
                    let (namespace, key) = path.split_once('.')?;
                    if namespace == "spacing" {
                        let base = self.raw("--spacing")?;
                        let number: f64 = key.parse().ok()?;
                        let unit =
                            base.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
                        let scale: f64 = base[..base.len() - unit.len()].parse().ok()?;
                        format!("{}{unit}", scale * number)
                    } else {
                        let namespace = match namespace {
                            "colors" => "--color",
                            "fontSize" => "--text",
                            "fontFamily" => "--font",
                            "fontWeight" => "--font-weight",
                            "letterSpacing" => "--tracking",
                            "lineHeight" => "--leading",
                            "borderRadius" => "--radius",
                            "boxShadow" => "--shadow",
                            "screens" => "--breakpoint",
                            "maxWidth" | "width" | "containers" => "--container",
                            "blur" => "--blur",
                            "animation" => "--animate",
                            "transitionTimingFunction" => "--ease",
                            _ => return None,
                        };
                        let key = key.replace('.', "-");
                        self.raw(&format!("{namespace}-{key}"))
                            .or_else(|| self.raw(&format!("--spacing-{key}")))?
                    }
                };
                Some(match alpha {
                    Some(alpha) => with_alpha(&value, alpha),
                    None => value,
                })
            }
        }
    }

    pub(super) fn spacing(&self, value: &str) -> Option<String> {
        if value.starts_with(['[', '(']) {
            return self.typed(value).map(|(_, raw)| raw);
        }
        if value == "px" {
            return Some("1px".into());
        }
        if let Some(value) = self.theme("--spacing", value) {
            return Some(value);
        }
        let number: f64 = value.parse().ok()?;
        (number >= 0.
            && (number * 4.).fract() == 0.
            && value.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        .then(|| self.function("--spacing", value))
        .flatten()
    }

    pub(super) fn color(&self, value: &str, modifier: Option<&str>) -> Option<String> {
        let color = if value.starts_with(['[', '(']) {
            let (kind, raw) = self.typed(value)?;
            if kind.as_deref().is_some_and(|kind| kind != "color")
                || kind.is_none() && !is_color(&raw) && !raw.starts_with("var(")
            {
                return None;
            }
            raw
        } else {
            match value {
                "inherit" => "inherit".into(),
                "current" => "currentcolor".into(),
                "transparent" => "transparent".into(),
                _ => self.theme("--color", value)?,
            }
        };
        let Some(modifier) = modifier else {
            return Some(color);
        };
        let alpha = self.opacity(modifier)?;
        (color != "inherit").then(|| with_alpha(&color, &alpha))
    }

    pub(super) fn opacity(&self, modifier: &str) -> Option<String> {
        if modifier.starts_with(['[', '(']) {
            return self.arbitrary(modifier);
        }
        let number: f64 = modifier.parse().ok()?;
        ((0. ..=100.).contains(&number) && (number * 4.).fract() == 0.)
            .then(|| format!("{modifier}%"))
    }

    pub(super) fn shadow(
        &self,
        value: &str,
        modifier: Option<&str>,
        alpha: &str,
    ) -> Option<String> {
        if matches!(value, "initial" | "inherit") && modifier.is_none() {
            return Some(value.into());
        }
        let color = self.color(value, modifier)?;
        Some(format!(
            "color-mix(in oklab, {color} var(--tw-{alpha}-alpha), transparent)"
        ))
    }
}

pub(super) fn statements(source: &str) -> Vec<(&str, Option<&str>)> {
    let mut items = Vec::new();
    let (mut start, mut open, mut depth, mut parens) = (0, 0, 0usize, 0usize);
    let mut chars = source.char_indices();
    while let Some((index, character)) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' | '\'' => skip_string(&mut chars, character),
            '(' | '[' => parens += 1,
            ')' | ']' => parens = parens.saturating_sub(1),
            '{' => {
                if depth == 0 {
                    open = index;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    items.push((source[start..open].trim(), Some(&source[open + 1..index])));
                    start = index + 1;
                }
            }
            ';' if depth == 0 && parens == 0 => {
                let item = source[start..index].trim();
                if !item.is_empty() {
                    items.push((item, None));
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let rest = source[start..].trim();
    if depth == 0 && !rest.is_empty() {
        items.push((rest, None));
    }
    items
}

pub(super) fn skip_string(chars: &mut std::str::CharIndices, quote: char) {
    let mut escaped = false;
    for (_, other) in chars.by_ref() {
        if escaped {
            escaped = false;
        } else if other == '\\' {
            escaped = true;
        } else if other == quote {
            break;
        }
    }
}

pub(super) fn call(text: &str, name: &str) -> Option<(usize, usize, usize)> {
    let (mut found, mut quote, mut escaped, mut depth) = (None, None, false, 0);
    for (index, character) in text.char_indices() {
        if let Some((_, open)) = found
            && index < open
        {
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quoted) = quote {
            if character == quoted {
                quote = None;
            }
            continue;
        }
        match (character, found) {
            ('"' | '\'', _) => quote = Some(character),
            ('(', Some(_)) => depth += 1,
            (')', Some((start, open))) if depth == 0 => return Some((start, open, index)),
            (')', Some(_)) => depth -= 1,
            (_, None)
                if text[index..].starts_with(name)
                    && text[index + name.len()..].starts_with('(') =>
            {
                found = Some((index, index + name.len() + 1));
            }
            _ => {}
        }
    }
    found.map(|(start, open)| (start, open, text.len()))
}

pub(super) fn calls(value: &str) -> bool {
    value.contains("theme(") || value.contains("--spacing(") || value.contains("--alpha(")
}

pub(super) fn split(value: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut stack = Vec::new();
    let mut start = 0;
    let mut chars = value.char_indices();
    while let Some((index, character)) = chars.next() {
        if character == separator && stack.is_empty() {
            parts.push(&value[start..index]);
            start = index + character.len_utf8();
            continue;
        }
        match character {
            '\\' => {
                chars.next();
            }
            '"' | '\'' => skip_string(&mut chars, character),
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            '{' => stack.push('}'),
            ')' | ']' | '}' if stack.last() == Some(&character) => {
                stack.pop();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

pub(super) fn modifier(value: &str) -> (&str, Option<&str>) {
    match split(value, '/').as_slice() {
        [value, modifier] if !value.is_empty() && !modifier.is_empty() => (value, Some(modifier)),
        _ => (value, None),
    }
}

pub(super) fn valid(value: &str) -> bool {
    let mut stack = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' if chars.next().is_none() => return false,
            '"' | '\'' => loop {
                match chars.next() {
                    None | Some('\n') => return false,
                    Some('\\') if chars.next().is_none() => return false,
                    Some(other) if other == character => break,
                    _ => {}
                }
            },
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            '{' => stack.push('}'),
            ')' | ']' | '}' if stack.pop() != Some(character) => return false,
            ';' if stack.is_empty() => return false,
            '/' if chars.peek() == Some(&'*') => return false,
            _ => {}
        }
    }
    stack.is_empty()
}

pub(super) fn named_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'%' | b'-'))
}

pub(super) fn integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn decode(raw: &str) -> String {
    let mut value = String::with_capacity(raw.len());
    let mut stack: Vec<bool> = Vec::new();
    let (mut word, mut first) = (0, false);
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if stack.last() == Some(&true) {
            match character {
                '(' => stack.push(true),
                ')' => {
                    stack.pop();
                }
                _ => {}
            }
            value.push(character);
            continue;
        }
        match character {
            '\\' if chars.peek() == Some(&'_') => {
                chars.next();
                value.push('_');
            }
            '\\' => {
                value.push(character);
                value.extend(chars.next());
            }
            '_' => value.push(if first { '_' } else { ' ' }),
            '"' | '\'' => {
                value.push(character);
                while let Some(other) = chars.next() {
                    match other {
                        '\\' if chars.peek() == Some(&'_') => {
                            chars.next();
                            value.push('_');
                        }
                        '\\' => {
                            value.push(other);
                            value.extend(chars.next());
                        }
                        '_' => value.push(' '),
                        _ => value.push(other),
                    }
                    if other == character {
                        break;
                    }
                }
            }
            '(' => {
                let name = &value[word..];
                let url = name == "url" || name.ends_with(" url");
                first = matches!(name, "var" | "theme")
                    || name.ends_with(" var")
                    || name.ends_with(" theme");
                stack.push(url);
                value.push(character);
                word = value.len();
            }
            ')' | ':' | ',' | '=' | '>' | '<' | '\n' | ' ' | '\t' | '/' => {
                if character == ')' {
                    stack.pop();
                }
                value.push(character);
                word = value.len();
                first = false;
            }
            _ => value.push(character),
        }
    }
    if !value.contains('(') {
        return value;
    }
    math(&value)
}

pub(super) const MATH: &[&str] = &[
    "calc", "min", "max", "clamp", "mod", "rem", "sin", "cos", "tan", "asin", "acos", "atan",
    "atan2", "pow", "sqrt", "hypot", "log", "exp", "round",
];

pub(super) fn math(value: &str) -> String {
    if !MATH.iter().any(|name| value.contains(name)) {
        return value.to_owned();
    }
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::with_capacity(value.len() + 8);
    let mut stack: Vec<bool> = Vec::new();
    let (mut number, mut previous) = (None, None);
    for (index, &character) in chars.iter().enumerate() {
        if character.is_ascii_digit()
            || number.is_some() && (character == '%' || character.is_ascii_alphabetic())
        {
            number = Some(index);
        } else {
            previous = number;
            number = None;
        }
        let math = stack.last() == Some(&true);
        match character {
            '(' => {
                result.push(character);
                let start = chars[..index]
                    .iter()
                    .rposition(|c| !(c.is_ascii_digit() || c.is_ascii_lowercase()))
                    .map_or(0, |position| position + 1);
                let name: String = chars[start..index].iter().collect();
                stack.push(MATH.contains(&name.as_str()) || math && name.is_empty());
            }
            ')' => {
                result.push(character);
                stack.pop();
            }
            ',' if math => result.push_str(", "),
            ' ' if math && result.ends_with(' ') => {}
            '+' | '*' | '/' | '-' if math => {
                let trimmed = result.trim_end();
                let mut tail = trimmed.chars().rev();
                let (last, before) = (tail.next(), tail.next());
                let next = chars.get(index + 1).copied();
                if matches!(last, Some('e' | 'E')) && before.is_some_and(|c| c.is_ascii_digit())
                    || matches!(last, Some('+' | '*' | '/' | '-' | '(' | ','))
                {
                    result.push(character);
                } else if index > 0 && chars[index - 1] == ' ' {
                    result.push(character);
                    result.push(' ');
                } else if last.is_some_and(|c| c.is_ascii_digit() || c == ')')
                    || next.is_some_and(|c| {
                        c.is_ascii_digit() || matches!(c, '(' | '+' | '*' | '/' | '-')
                    })
                    || index > 0 && previous == Some(index - 1)
                {
                    result.push(' ');
                    result.push(character);
                    result.push(' ');
                } else {
                    result.push(character);
                }
            }
            _ => result.push(character),
        }
    }
    result
}

pub(super) fn with_alpha(color: &str, alpha: &str) -> String {
    let alpha = match alpha.parse::<f64>() {
        Ok(number) => format!("{}%", number * 100.),
        Err(_) => alpha.to_owned(),
    };
    if alpha == "100%" {
        return color.to_owned();
    }
    format!("color-mix(in oklab, {color} {alpha}, transparent)")
}

pub(super) fn universal(selector: &str, all: bool) -> String {
    let mut result = String::with_capacity(selector.len() + 2);
    let mut chars = selector.chars().peekable();
    let mut done = false;
    while let Some(character) = chars.next() {
        if character != '&' || done {
            result.push(character);
            continue;
        }
        done = !all;
        if !matches!(chars.peek(), Some(':' | '[' | '.' | '#')) {
            result.push('*');
        }
    }
    result
}

pub(super) fn negate(rule: &str) -> Option<String> {
    let (name, params) = rule.split_once(' ')?;
    let params = params.trim();
    if !matches!(name, "@media" | "@supports" | "@container") || split(params, ',').len() > 1 {
        return None;
    }
    let words: Vec<_> = split(params, ' ')
        .into_iter()
        .filter(|word| !word.is_empty())
        .collect();
    Some(match words.as_slice() {
        ["not", rest @ ..] => format!("{name} {}", rest.join(" ")),
        [container, "not", rest @ ..] if name == "@container" && !container.starts_with('(') => {
            format!("{name} {container} {}", rest.join(" "))
        }
        [container, rest @ ..] if name == "@container" && !container.starts_with('(') => {
            format!("{name} {container} not {}", rest.join(" "))
        }
        _ => format!("{name} not {params}"),
    })
}

pub(super) fn removes(cleared: &str, name: &str) -> bool {
    match cleared.strip_suffix("-*") {
        Some(namespace) => name.starts_with(namespace) && !kept(name, namespace),
        None => cleared == name,
    }
}

pub(super) fn kept(name: &str, namespace: &str) -> bool {
    KEPT.iter().any(|(other, names)| {
        *other == namespace
            && names.iter().any(|kept| {
                name.strip_prefix(kept)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
            })
    })
}

pub(super) fn direction(side: &str) -> Option<&'static str> {
    Some(match side {
        "t" => "top",
        "tr" => "top right",
        "r" => "right",
        "br" => "bottom right",
        "b" => "bottom",
        "bl" => "bottom left",
        "l" => "left",
        "tl" => "top left",
        _ => return None,
    })
}

pub(super) fn is_color(value: &str) -> bool {
    value.starts_with('#')
        || [
            "rgb(",
            "rgba(",
            "hsl(",
            "hsla(",
            "hwb(",
            "lab(",
            "lch(",
            "oklab(",
            "oklch(",
            "color(",
            "color-mix(",
            "light-dark(",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
        || value.eq_ignore_ascii_case("currentcolor")
        || value == "transparent"
        || color(value).is_some()
}

pub(super) fn is_type(kind: &str, value: &str) -> bool {
    match kind {
        "number" => value.parse::<f64>().is_ok_and(|number| number >= 0.),
        "integer" => value.parse::<u64>().is_ok(),
        "percentage" => value
            .strip_suffix('%')
            .is_some_and(|number| number.parse::<f64>().is_ok()),
        "length" => is_length(value) && !value.ends_with('%'),
        "duration" => {
            value.ends_with("ms")
                || value
                    .strip_suffix('s')
                    .is_some_and(|number| number.parse::<f64>().is_ok())
        }
        "color" => is_color(value),
        "ratio" => value
            .split_once('/')
            .is_some_and(|(a, b)| integer(a.trim()) && integer(b.trim())),
        _ => false,
    }
}

// Tailwind's inference of arbitrary background and mask values: "position" or "size".
pub(super) fn layer(raw: &str) -> Option<&'static str> {
    let length = |part: &str| {
        is_length(part) && (part == "0" || part.parse::<f64>().is_err() || part.ends_with('%'))
    };
    let parts = || {
        raw.split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|part| !part.is_empty())
    };
    if parts()
        .all(|part| length(part) || matches!(part, "left" | "right" | "top" | "bottom" | "center"))
    {
        return Some("position");
    }
    parts()
        .all(|part| length(part) || matches!(part, "auto" | "cover" | "contain"))
        .then_some("size")
}

pub(super) fn is_length(value: &str) -> bool {
    value.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
        || ["calc(", "min(", "max(", "clamp("]
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

pub(super) fn shadow_color(shadow: &str, name: &str, alpha: Option<&str>) -> String {
    let replacement = |color: &str| match alpha {
        Some(alpha) => format!("var({name}, oklab(from {color} l a b / {alpha}))"),
        None => format!("var({name}, {color})"),
    };
    split(shadow, ',')
        .into_iter()
        .map(|layer| {
            let layer = layer.trim();
            let (mut color, mut offsets) = (None, 0);
            for part in split(layer, ' ') {
                if part.is_empty()
                    || matches!(part, "inset" | "inherit" | "initial" | "revert" | "unset")
                {
                    continue;
                }
                let digits = part.strip_prefix('-').unwrap_or(part);
                if digits.starts_with(|c: char| c.is_ascii_digit())
                    || digits.starts_with('.')
                        && digits[1..].starts_with(|c: char| c.is_ascii_digit())
                {
                    offsets += 1;
                } else if color.is_none() {
                    color = Some(part);
                }
            }
            match (offsets, color) {
                (0 | 1, _) => layer.to_owned(),
                (_, Some(color)) => layer.replacen(color, &replacement(color), 1),
                _ => format!("{layer} {}", replacement("currentcolor")),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn quote(value: &str) -> String {
    let Some((name, rest)) = value.split_once('=') else {
        return value.to_owned();
    };
    let rest = rest.trim();
    if rest.starts_with(['\'', '"']) {
        return value.to_owned();
    }
    format!("{name}=\"{rest}\"")
}

pub(super) fn escape(class: &str) -> String {
    let mut selector = String::with_capacity(class.len() + 8);
    selector.push('.');
    for (index, character) in class.chars().enumerate() {
        if index == 0 && character.is_ascii_digit() {
            let _ = write!(selector, "\\{:x} ", character as u32);
        } else if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_')
            || !character.is_ascii()
        {
            selector.push(character);
        } else {
            selector.push('\\');
            selector.push(character);
        }
    }
    selector
}
