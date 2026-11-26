use super::utility::Modifier;
use super::*;
use crate::style::tailwind::tables::{DEFAULTS, INLINE, KEPT};

impl Config {
    pub(super) fn removed(&self, name: &str) -> bool {
        self.cleared.iter().any(|cleared| removes(cleared, name))
    }

    pub(super) fn raw(&self, name: &str) -> Option<&str> {
        if let Some(theme) = self.theme.get(name) {
            return Some(&theme.value);
        }
        if self.imports[THEME].is_none() && !self.referenced || self.removed(name) {
            return None;
        }
        DEFAULTS.get(name).copied()
    }

    pub(super) fn flags(&self, name: &str) -> u8 {
        let own = match self.theme.get(name) {
            Some(theme) => theme.options,
            None if INLINE.contains(&name) => INLINE_VALUE | REFERENCE,
            None => self.options,
        };
        own | if self.imports[THEME].is_none() && self.imports[UTILITIES].is_none() {
            REFERENCE
        } else {
            0
        }
    }

    // The value of a theme variable in a declaration.
    pub(super) fn variable(&self, name: &str) -> Option<String> {
        let value = self.raw(name)?;
        let flags = self.flags(name);
        if flags & INLINE_VALUE != 0 {
            return Some(value.to_owned());
        }
        let prefixed;
        let name = match self.prefix.as_str() {
            "" => name,
            _ => {
                prefixed = self.prefixed(name);
                &prefixed
            }
        };
        let name = escape_name(name);
        Some(match flags & REFERENCE {
            0 => format!("var({name})"),
            _ => format!("var({name}, {value})"),
        })
    }

    pub(super) fn key(&self, namespaces: &str, key: &str) -> Option<String> {
        let mut name = String::new();
        let find = |namespace: &&str| {
            // The default theme has no names in the other namespaces.
            if self.theme.is_empty() && !named(namespace) {
                return false;
            }
            name.clear();
            name.push_str(namespace);
            name.push('-');
            let start = name.len();
            name.push_str(key);
            if self.raw(&name).is_none() {
                if !key.contains('.') {
                    return false;
                }
                name.truncate(start);
                name.push_str(&key.replace('.', "_"));
                if self.raw(&name).is_none() {
                    return false;
                }
            }
            !kept(&name, namespace)
        };
        namespaces.split(' ').find(find)?;
        Some(name)
    }

    pub(super) fn themed(&self, namespaces: &str, key: &str) -> Option<String> {
        self.variable(&self.key(namespaces, key)?)
    }

    pub(super) fn namespace(&self, namespace: &str) -> impl Iterator<Item = &str> {
        let names: BTreeSet<_> = DEFAULTS
            .keys()
            .copied()
            .chain(self.theme.keys().map(String::as_str))
            .filter(|name| {
                name.strip_prefix(namespace)
                    .is_some_and(|rest| rest.starts_with('-'))
            })
            .collect();
        names.into_iter().filter_map(|name| self.raw(name))
    }

    pub(super) fn prefixed(&self, name: &str) -> String {
        match self.prefix.as_str() {
            "" => name.to_owned(),
            prefix => format!("--{prefix}-{}", &name[2..]),
        }
    }

    // The theme name of a variable in CSS, whose name comes without its dashes.
    pub(super) fn unprefixed(&self, name: &str) -> Option<String> {
        let name = match self.prefix.as_str() {
            "" => name,
            prefix => name.strip_prefix(prefix)?.strip_prefix('-')?,
        };
        Some(format!("--{name}"))
    }

    pub(super) fn alpha(&self, color: String, modifier: Option<&Modifier>) -> Option<String> {
        Some(match modifier {
            None => color,
            Some(Modifier::Raw(alpha)) => with_alpha(&color, alpha),
            Some(Modifier::Named(name)) => match self.themed("--opacity", name) {
                Some(alpha) => with_alpha(&color, &alpha),
                None if quarter(name) => with_alpha(&color, &format!("{name}%")),
                None => return None,
            },
        })
    }

    // The value of `--value()` or `--modifier()` in a custom utility, and whether it is a ratio.
    pub(super) fn resolve(
        &self,
        arguments: &str,
        value: Option<&str>,
        fraction: Option<&str>,
    ) -> Option<(String, bool)> {
        let arguments = || split(arguments, ',').into_iter().map(str::trim);
        let Some(value) = value else {
            return arguments().find_map(|argument| {
                let default = argument.strip_prefix("--default(")?.strip_suffix(')')?;
                Some((default.to_owned(), false))
            });
        };
        let typed = match value.starts_with(['[', '(']) {
            true => Some(self.typed(value)?),
            false => None,
        };
        arguments().find_map(|argument| {
            if argument.starts_with("--default(") {
                return None;
            }
            let quoted = argument.trim_matches(['"', '\'']);
            let argument: String = match quoted.len() < argument.len() {
                true => argument.to_owned(),
                false => argument
                    .replace('\\', "")
                    .split_ascii_whitespace()
                    .collect(),
            };
            if let Some((hint, raw)) = &typed {
                let kind = argument.strip_prefix('[')?.strip_suffix(']')?;
                let matches = kind == "*"
                    || match hint {
                        Some(hint) => hint == kind,
                        None => infer(raw, kind).is_some(),
                    };
                return matches.then(|| (raw.clone(), false));
            }
            if quoted.len() < argument.len() && !argument.starts_with('-') {
                return (quoted == value).then(|| (value.to_owned(), false));
            }
            if argument.starts_with("--") {
                let (namespace, option) =
                    argument.split_once("-*").unwrap_or((argument.as_str(), ""));
                let name = self.key(namespace, value)?;
                return Some((self.variable(&format!("{name}{option}"))?, false));
            }
            if argument == "ratio" {
                let (a, b) = (value, fraction?);
                return (integer(a) && integer(b)).then(|| (format!("{a} / {b}"), true));
            }
            let valid = match argument.as_str() {
                "number" => quarter(value),
                "integer" => integer(value),
                "percentage" => value.strip_suffix('%').is_some_and(integer),
                _ => false,
            };
            valid.then(|| (value.to_owned(), false))
        })
    }

    pub(super) fn typed(&self, value: &str) -> Option<(Option<String>, String)> {
        let raw = arbitrary(value)?;
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

    // The value of a class with its calls resolved. A call that Tailwind rejects makes the class invalid.
    pub(super) fn resolved(&self, value: &str) -> Option<String> {
        if !calls(value) {
            return Some(value.to_owned());
        }
        let mut failed = false;
        let value = self.replace(value, &mut failed);
        (!failed).then_some(value)
    }

    pub(super) fn functions(&self, value: &str) -> String {
        if !calls(value) {
            return value.to_owned();
        }
        self.replace(value, &mut false)
    }

    fn replace(&self, value: &str, failed: &mut bool) -> String {
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
                        match self.function(function, arguments.trim()) {
                            Some(result) => {
                                out.truncate(start);
                                out.push_str(&result);
                            }
                            None => *failed = true,
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
                let base = self.variable("--spacing")?;
                let end = number_end(arguments).unwrap_or(0);
                let unit = &arguments[end..];
                let number = match unit.bytes().all(|b| b.is_ascii_alphabetic()) || unit == "%" {
                    true => arguments[..end].parse::<f64>().ok(),
                    false => None,
                };
                Some(match number {
                    _ if arguments.is_empty() || split(arguments, ',').len() > 1 => return None,
                    Some(0.) => "0px".into(),
                    Some(1.) => base,
                    _ => format!("calc({base} * {arguments})"),
                })
            }
            "--alpha" => {
                let [color, alpha] = split(arguments, '/')[..] else {
                    return None;
                };
                let (color, alpha) = (color.trim(), alpha.trim());
                (!color.is_empty() && !alpha.is_empty()).then(|| with_alpha(color, alpha))
            }
            "--theme" => {
                let parts = split(arguments, ',');
                let name = parts[0].trim();
                let fallback =
                    (parts.len() > 1).then(|| arguments[parts[0].len() + 1..].trim().to_owned());
                let (name, inline) = match name.strip_suffix(" inline") {
                    Some(name) => (name.trim(), true),
                    None => (name, false),
                };
                let (name, alpha) = match name.rsplit_once('/') {
                    Some((name, alpha)) => (name.trim(), Some(alpha.trim())),
                    None => (name, None),
                };
                if !name.starts_with("--") {
                    return None;
                }
                let value = match inline {
                    true => self.raw(name).map(str::to_owned),
                    false => self.variable(name),
                };
                let Some(value) = value else {
                    return fallback;
                };
                let value = match alpha {
                    Some(alpha) => with_alpha(&value, alpha),
                    None => value,
                };
                Some(match fallback.as_deref() {
                    Some(fallback) if fallback != "initial" => {
                        match value
                            .strip_suffix(')')
                            .filter(|_| value.starts_with("var("))
                        {
                            Some(open) if split(open, ',').len() == 1 && !open.contains(", ") => {
                                format!("{open}, {fallback})")
                            }
                            _ => value,
                        }
                    }
                    _ => value,
                })
            }
            _ => {
                // Author CSS separates `spacing` and `.4` with `/**/` when it is serialized.
                let arguments = arguments.replace("/**/", "");
                let parts = split(&arguments, ',');
                let path = parts[0].trim();
                let quoted = path.trim_matches(['"', '\'']);
                let fallback =
                    || (parts.len() > 1).then(|| arguments[parts[0].len() + 1..].trim().to_owned());
                let (path, alpha) = match quoted.rsplit_once('/') {
                    Some((path, alpha)) => (path.trim(), Some(alpha.trim())),
                    None => (quoted, None),
                };
                let Some(value) = self.path(path) else {
                    return fallback();
                };
                Some(match alpha {
                    Some(alpha) => with_alpha(&value, alpha),
                    None => value,
                })
            }
        }
    }

    // A value of the theme by its name or by a path of Tailwind 3, like `colors.red.500`.
    fn path(&self, path: &str) -> Option<String> {
        if path.starts_with("--") {
            return self.raw(path).map(str::to_owned);
        }
        let path = path.replace('[', ".").replace(']', "");
        let (namespace, key) = path.split_once('.')?;
        let mut name = String::from("--");
        for character in namespace.chars() {
            if character.is_ascii_uppercase() {
                name.push('-');
            }
            name.push(character.to_ascii_lowercase());
        }
        let alias = match namespace {
            "animation" => "--animate",
            "aspectRatio" => "--aspect",
            "borderRadius" => "--radius",
            "boxShadow" => "--shadow",
            "colors" | "backgroundColor" | "textColor" | "borderColor" | "fill" | "stroke" => {
                "--color"
            }
            "containers" | "maxWidth" | "width" | "columns" => "--container",
            "fontFamily" => "--font",
            "fontSize" => "--text",
            "letterSpacing" => "--tracking",
            "lineHeight" => "--leading",
            "screens" => "--breakpoint",
            "transitionTimingFunction" => "--ease",
            _ => "--spacing",
        };
        let mut rest = String::new();
        for part in key.split('.').filter(|part| *part != "DEFAULT") {
            rest.push('-');
            rest.push_str(match part {
                "lineHeight" => "-line-height",
                "letterSpacing" => "-letter-spacing",
                "fontWeight" => "-font-weight",
                part => part,
            });
        }
        for namespace in [name.as_str(), alias] {
            let dotted = format!("{namespace}-{}", key.replace('.', "_"));
            let value = self.raw(&format!("{namespace}{rest}"));
            if let Some(value) = value.or_else(|| self.raw(&dotted)) {
                return Some(value.to_owned());
            }
        }
        if alias != "--spacing" && namespace != "width" {
            return None;
        }
        let base = self.raw("--spacing")?;
        let end = number_end(base)?;
        let (scale, number): (f64, f64) = (base[..end].parse().ok()?, key.parse().ok()?);
        quarter(key).then(|| format!("{}{}", scale * number, &base[end..]))
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
            '{' if stack.is_empty() => return false,
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

// A whole number as JavaScript writes it, so `01` is none.
pub(super) fn integer(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 16
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
}

// A multiple of 0.25 as JavaScript writes it.
pub(super) fn quarter(value: &str) -> bool {
    match value.split_once('.') {
        Some((whole, part)) => integer(whole) && matches!(part, "25" | "5" | "75"),
        None => integer(value),
    }
}

// Whether a margin is zero, like `0px`.
pub(super) fn zero(value: &str) -> bool {
    number_end(value).is_some_and(|end| {
        value[..end].parse() == Ok(0.) && (end == value.len() || UNITS.contains(&&value[end..]))
    })
}

pub(super) fn arbitrary(value: &str) -> Option<String> {
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
    (valid(&raw) && !raw.trim().is_empty()).then_some(raw)
}

// The classes that a pattern like `p-{1,2}` or `m-{0..4..2}` stands for, up to a limit.
pub(super) fn braces(pattern: &str) -> Vec<String> {
    const LIMIT: usize = 1 << 14;
    fn expand(pattern: &str, depth: u8, work: &mut usize) -> Vec<String> {
        let Some(open) = pattern.find('{').filter(|_| depth < 8) else {
            return vec![pattern.to_owned()];
        };
        let mut level = 0;
        let close = pattern[open..]
            .char_indices()
            .find_map(|(index, character)| {
                level += i32::from(character == '{') - i32::from(character == '}');
                (level == 0).then_some(open + index)
            });
        let Some(close) = close else {
            return Vec::new();
        };
        let inner = &pattern[open + 1..close];
        let numbers: Option<Vec<i64>> = inner.split("..").map(|part| part.parse().ok()).collect();
        let mut parts = Vec::new();
        match numbers.as_deref() {
            Some([start, end, rest @ ..]) if rest.len() < 2 => {
                let step = rest.first().copied().unwrap_or(1).unsigned_abs().max(1);
                let count = start.abs_diff(*end) / step;
                let sign = if start <= end { 1 } else { -1 };
                let list = (0..=count).take(LIMIT - (*work).min(LIMIT));
                parts.extend(list.map(|index| {
                    (i128::from(*start) + sign * i128::from(index) * i128::from(step)).to_string()
                }));
                *work += parts.len();
            }
            _ => {
                for part in split(inner, ',') {
                    parts.extend(expand(part, depth + 1, work));
                }
            }
        }
        let tail = expand(&pattern[close + 1..], depth + 1, work);
        let mut out = Vec::new();
        let pairs = tail
            .iter()
            .flat_map(|rest| parts.iter().map(move |part| (part, rest)));
        for (part, rest) in pairs {
            *work += 1;
            if *work > LIMIT {
                break;
            }
            out.push(format!("{}{part}{rest}", &pattern[..open]));
        }
        out
    }
    expand(pattern, 0, &mut 0)
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

pub(super) fn percent(alpha: &str) -> std::borrow::Cow<'_, str> {
    match alpha.parse::<f64>() {
        Ok(number) if number.is_finite() => format!("{}%", number * 100.).into(),
        _ => alpha.into(),
    }
}

pub(super) fn with_alpha(color: &str, alpha: &str) -> String {
    let alpha = percent(alpha);
    if alpha == "100%" {
        return color.to_owned();
    }
    format!("color-mix(in oklab, {color} {alpha}, transparent)")
}

// Puts a selector in the place of `&`. Like CSS nesting, a selector with combinators stays one part behind
// another one.
pub(super) fn nest(template: &str, selector: &str) -> String {
    if let Some(rest) = template.strip_prefix('&')
        && !rest.contains('&')
    {
        return format!("{selector}{rest}");
    }
    let complex = || {
        let mut depth = 0;
        let mut chars = selector.chars();
        while let Some(character) = chars.next() {
            match character {
                '\\' => {
                    chars.next();
                }
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                ' ' | '>' | '+' | '~' if depth == 0 => return true,
                _ => {}
            }
        }
        false
    };
    let first = |index: usize| {
        let before = template[..index].trim_end();
        before.is_empty() || before.ends_with(['(', ','])
    };
    if template.match_indices('&').all(|(index, _)| first(index)) || !complex() {
        return template.replace('&', selector);
    }
    let (mut result, mut end) = (String::new(), 0);
    for (index, _) in template.match_indices('&') {
        result.push_str(&template[end..index]);
        match first(index) {
            true => result.push_str(selector),
            false => {
                let _ = write!(result, ":is({selector})");
            }
        }
        end = index + 1;
    }
    result + &template[end..]
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
        [container, "not", rest @ ..] if name == "@container" && !container.contains('(') => {
            format!("{name} {container} {}", rest.join(" "))
        }
        [container, rest @ ..] if name == "@container" && !container.contains('(') => {
            format!("{name} {container} not {}", rest.join(" "))
        }
        _ => format!("{name} not {params}"),
    })
}

fn named(namespace: &str) -> bool {
    matches!(
        namespace,
        "--animate"
            | "--aspect"
            | "--blur"
            | "--breakpoint"
            | "--color"
            | "--container"
            | "--drop-shadow"
            | "--ease"
            | "--font"
            | "--font-weight"
            | "--inset-shadow"
            | "--leading"
            | "--max-width"
            | "--perspective"
            | "--radius"
            | "--shadow"
            | "--text"
            | "--text-shadow"
            | "--tracking"
    )
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

const UNITS: &[&str] = &[
    "cm", "mm", "Q", "in", "pc", "pt", "px", "em", "ex", "ch", "cap", "ic", "rem", "rex", "rch",
    "rcap", "ric", "lh", "rlh", "vw", "vh", "vmin", "vmax", "vb", "vi", "svw", "svh", "svi", "svb",
    "svmin", "svmax", "lvw", "lvh", "lvi", "lvb", "lvmin", "lvmax", "dvw", "dvh", "dvi", "dvb",
    "dvmin", "dvmax", "cqw", "cqh", "cqi", "cqb", "cqmin", "cqmax",
];

const COLOR_FUNCTIONS: &[&str] = &[
    "--alpha",
    "color",
    "color-mix",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "light-dark",
    "oklab",
    "oklch",
    "rgb",
    "rgba",
];

pub(super) fn is_color(value: &str) -> bool {
    if value.starts_with('#') {
        return true;
    }
    if let Some((name, _)) = value.split_once('(') {
        return COLOR_FUNCTIONS
            .iter()
            .any(|function| name.eq_ignore_ascii_case(function));
    }
    value.bytes().all(|b| b.is_ascii_alphabetic())
        && (value.eq_ignore_ascii_case("currentcolor") || color(value).is_some())
}

// Where the number at the start of `value` ends.
pub(super) fn number_end(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(end);
    end += whole;
    let part = match bytes.get(end) {
        Some(b'.') => digits(end + 1),
        _ => 0,
    };
    if part > 0 {
        end += part + 1;
    } else if whole == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits((end + 1 + sign).min(bytes.len()));
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    Some(end)
}

// The first of the types that Tailwind infers for an arbitrary value.
pub(super) fn infer<'a>(value: &str, types: &'a str) -> Option<&'a str> {
    if value.starts_with("var(") {
        return None;
    }
    let math = |value: &str| {
        value.contains('(')
            && MATH
                .iter()
                .any(|name| value.contains(name) && value.contains(&format!("{name}(")))
    };
    let unit = |value: &str| number_end(value).map(|end| value[end..].to_owned());
    let number = |value: &str| unit(value).is_some_and(|unit| unit.is_empty()) || math(value);
    let percentage = |value: &str| unit(value).is_some_and(|unit| unit == "%") || math(value);
    let length = |value: &str| {
        unit(value).is_some_and(|unit| UNITS.contains(&unit.as_str()))
            || value
                .get(..10)
                .is_some_and(|start| start.eq_ignore_ascii_case("--spacing("))
            || math(value)
    };
    let url = |value: &str| value.starts_with("url(") && value.ends_with(')');
    types.split(' ').find(|kind| match *kind {
        "color" => is_color(value),
        "length" => length(value),
        "percentage" => percentage(value),
        "number" => number(value),
        "integer" => integer(value),
        "ratio" => {
            math(value)
                || value.split_once('/').is_some_and(|(a, b)| {
                    let plain = |part: &str| unit(part).is_some_and(|unit| unit.is_empty());
                    plain(a.trim_end()) && plain(b.trim_start())
                })
        }
        "url" => url(value),
        "position" => {
            let mut found = false;
            split(value, ' ').into_iter().all(|part| {
                let known = matches!(part, "center" | "top" | "right" | "bottom" | "left")
                    || !part.starts_with("var(") && (length(part) || percentage(part));
                found |= known;
                known || part.starts_with("var(")
            }) && found
        }
        "bg-size" => {
            let mut found = false;
            split(value, ',').into_iter().all(|layer| {
                let parts = split(layer, ' ');
                let fits = matches!(layer, "cover" | "contain");
                found |= fits
                    || parts.len() < 3
                        && parts
                            .iter()
                            .all(|part| *part == "auto" || length(part) || percentage(part));
                fits || parts.len() < 3
            }) && found
        }
        "line-width" => split(value, ' ').into_iter().all(|part| {
            length(part) || number(part) || matches!(part, "thin" | "medium" | "thick")
        }),
        "image" => {
            let mut found = false;
            split(value, ',').into_iter().all(|part| {
                let name = part.split('(').next().unwrap_or_default();
                let gradient = name.strip_prefix("repeating-").unwrap_or(name);
                let known = part.contains('(')
                    && (matches!(
                        gradient,
                        "conic-gradient" | "linear-gradient" | "radial-gradient"
                    ) || matches!(name, "element" | "image" | "cross-fade" | "image-set")
                        || url(part));
                found |= known;
                known || part.starts_with("var(")
            }) && found
        }
        "family-name" => {
            let parts = split(value, ',');
            parts
                .iter()
                .all(|part| !part.starts_with(|c: char| c.is_ascii_digit()))
                && parts.iter().any(|part| !part.starts_with("var("))
        }
        "generic-name" => matches!(
            value,
            "serif"
                | "sans-serif"
                | "monospace"
                | "cursive"
                | "fantasy"
                | "system-ui"
                | "ui-serif"
                | "ui-sans-serif"
                | "ui-monospace"
                | "ui-rounded"
                | "math"
                | "emoji"
                | "fangsong"
        ),
        "absolute-size" => matches!(
            value,
            "xx-small"
                | "x-small"
                | "small"
                | "medium"
                | "large"
                | "x-large"
                | "xx-large"
                | "xxx-large"
        ),
        "relative-size" => matches!(value, "larger" | "smaller"),
        "angle" => {
            unit(value).is_some_and(|unit| matches!(unit.as_str(), "deg" | "rad" | "grad" | "turn"))
        }
        "vector" => {
            let parts: Vec<_> = value.split(' ').filter(|part| !part.is_empty()).collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| unit(part).is_some_and(|unit| unit.is_empty()))
        }
        _ => false,
    })
}

// One layer of a shadow with its color replaced by the variable `name`.
pub(super) fn shadow_color(layer: &str, name: &str, alpha: Option<&str>) -> String {
    let replacement = |color: &str| match alpha {
        Some(alpha) if color.starts_with("current") => {
            format!("var({name}, {})", with_alpha(color, alpha))
        }
        Some(alpha) => {
            let alpha = percent(alpha);
            format!("var({name}, oklab(from {color} l a b / {alpha}))")
        }
        None => format!("var({name}, {color})"),
    };
    let (mut other, mut others, mut offsets) = ("", 0, 0);
    let swap = |part: &str| {
        let at = part.as_ptr() as usize - layer.as_ptr() as usize;
        let end = at + part.len();
        format!("{}{}{}", &layer[..at], replacement(part), &layer[end..])
    };
    for part in split(layer, ' ') {
        let lower = part.to_ascii_lowercase();
        let digits = lower.strip_prefix('-').unwrap_or(&lower);
        if part.is_empty()
            || matches!(
                lower.as_str(),
                "inset" | "inherit" | "initial" | "revert" | "unset"
            )
        {
        } else if digits.starts_with(|c: char| c.is_ascii_digit())
            || digits.starts_with('.') && digits[1..].starts_with(|c: char| c.is_ascii_digit())
            || ["calc(", "clamp(", "max(", "min(", "--spacing("]
                .iter()
                .any(|function| lower.starts_with(function))
        {
            offsets += 1;
        } else if is_color(part)
            || lower.starts_with("contrast-color(")
            || lower.starts_with("device-cmyk(")
        {
            return swap(part);
        } else {
            others += 1;
            other = part;
        }
    }
    match (offsets, others) {
        (0 | 1, _) => layer.to_owned(),
        (_, 0) => format!("{layer} {}", replacement("currentcolor")),
        (_, 1) => swap(other),
        _ => layer.to_owned(),
    }
}

pub(super) fn escape_name(name: &str) -> std::borrow::Cow<'_, str> {
    if named_value(name) && !name.contains(['.', '%']) {
        return name.into();
    }
    let mut escaped = String::with_capacity(name.len() + 2);
    for character in name.chars() {
        if character.is_ascii() && !character.is_ascii_alphanumeric() && !"-_".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.into()
}

pub(super) fn quote(value: &str) -> String {
    let Some((name, rest)) = value.split_once('=') else {
        return value.to_owned();
    };
    let rest = rest.trim();
    if rest.starts_with(['\'', '"']) {
        return value.to_owned();
    }
    match rest.rsplit_once(' ') {
        Some((value, flag @ ("i" | "I" | "s" | "S"))) => format!("{name}=\"{value}\" {flag}"),
        _ => format!("{name}=\"{rest}\""),
    }
}

// Whether the text is an attribute selector, like `[data-state="open"]`.
pub(super) fn attribute(text: &str) -> bool {
    let Some(text) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) else {
        return false;
    };
    let text = text.trim();
    let mut end = 0;
    let mut chars = text.char_indices();
    while let Some((index, character)) = chars.next() {
        if character == '\\' {
            chars.next();
        } else if !(character.is_ascii_alphanumeric()
            || "-_".contains(character)
            || !character.is_ascii())
        {
            break;
        }
        end = index + character.len_utf8();
    }
    let rest = text[end.min(text.len())..].trim_start();
    if end == 0 || rest.is_empty() {
        return end > 0;
    }
    let rest = rest.strip_prefix(['~', '|', '^', '$', '*']).unwrap_or(rest);
    let Some(rest) = rest.strip_prefix('=') else {
        return false;
    };
    let rest = rest.trim();
    let rest = match rest.chars().next() {
        Some(quote @ ('"' | '\'')) => match rest.rfind(quote) {
            Some(close) if close > 0 => rest[close + 1..].trim(),
            _ => return false,
        },
        Some(_) => rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, flag)| flag.trim()),
        None => return false,
    };
    matches!(rest, "" | "i" | "I" | "s" | "S")
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
