use serde::{Deserialize, Serialize};
use std::{borrow::Cow, hash::BuildHasher, rc::Rc};

pub(crate) type SelectorList = Rc<Vec<Selector>>;

pub(crate) const FOCUS: u32 = 1;
pub(crate) const FOCUS_WITHIN: u32 = 2;
pub(crate) const HOVER: u32 = 4;
pub(crate) const ACTIVE: u32 = 8;
pub(crate) const FOCUS_VISIBLE: u32 = 128;
pub(crate) const HAS: u32 = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Selector {
    pub(crate) simple: Vec<SimpleSelector>,
    pub(crate) previous: Option<(char, Box<Selector>)>,
    pub(crate) specificity: [usize; 3],
    #[serde(skip)]
    pub(crate) ancestry: std::cell::OnceCell<Option<Ancestry>>,
}

pub(crate) const BLOOM: usize = 4;
pub(crate) type Bloom = [u64; BLOOM];
pub(crate) type Ancestry = (Bloom, Box<[u16]>, u16);

// Lowercase tags and attributes on both sides, or the filter rejects valid matches.
pub(crate) fn feature(bloom: &mut Bloom, kind: u8, value: &str) {
    let hash = rustc_hash::FxBuildHasher.hash_one((kind, value));
    for shift in [0, 32] {
        let bit = (hash >> shift) as usize % (64 * BLOOM);
        bloom[bit / 64] |= 1 << (bit % 64);
    }
}

pub(crate) fn lowercase(value: &str) -> Cow<'_, str> {
    match value.bytes().any(|c| c.is_ascii_uppercase()) {
        true => value.to_ascii_lowercase().into(),
        false => value.into(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum SimpleSelector {
    Tag(String),
    Id(String),
    Class(String),
    Universal,
    Root,
    // The flag is 1 for `i`, and 2 for the HTML attributes whose values ignore case on HTML elements.
    Attribute(String, String, String, u8),
    // The last field tells that the argument was parsed in an `@scope` rule, where `:scope` is its root.
    Pseudo(
        String,
        String,
        #[serde(skip)] Rc<[Selector]>,
        [i32; 2],
        bool,
    ),
    Element(String, String),
    Parent(#[serde(skip)] SelectorList, usize),
    NoNamespace,
}

thread_local! {
    static SCOPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) const PSEUDOS: [&str; 4] = [
    "active active-view-transition any-link autofill blank buffering checked closed corner-present decrement default defined disabled double-button empty enabled end first-child first-of-type focus focus-visible focus-within fullscreen future has-slotted horizontal host hover in-range increment indeterminate invalid last-child last-of-type link local-link modal muted no-button only-child only-of-type open optional out-of-range past paused picture-in-picture placeholder-shown playing popover-open read-only read-write required root scope seeking single-button stalled start target target-within user-invalid user-valid valid vertical visited volume-locked window-inactive -webkit-any-link -webkit-autofill -webkit-full-screen",
    "active-view-transition-type current dir has host host-context is lang not nth-child nth-last-child nth-last-of-type nth-of-type state where -webkit-any",
    "after backdrop before checkmark column cue details-content file-selector-button first-letter first-line grammar-error marker picker-icon placeholder scroll-marker scroll-marker-group search-text selection spelling-error starting-style target-text view-transition",
    "cue highlight part picker scroll-button slotted view-transition-group view-transition-image-pair view-transition-new view-transition-old",
];

// Their values match without case on HTML elements, from the HTML standard.
pub(crate) const HTML_INSENSITIVE: &str = "accept accept-charset align alink axis bgcolor charset checked clear codetype color compact declare defer dir direction disabled enctype face frame hreflang http-equiv lang language link media method multiple nohref noresize noshade nowrap readonly rel rev rules scope scrolling selected shape target text type valign valuetype vlink";

pub(crate) fn split(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = source;
    loop {
        let end = selector_boundary(rest, |c| c == ',');
        parts.push(&rest[..end]);
        if end == rest.len() {
            return parts;
        }
        rest = &rest[end + 1..];
    }
}

pub(crate) fn unescape(source: &str, stop: impl Fn(char) -> bool) -> (String, usize) {
    let mut value = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c != '\\' {
            if stop(c) {
                return (value, index);
            }
            value.push(c);
            continue;
        }
        let (mut code, mut digits) = (0, 0);
        while digits < 6
            && let Some(digit) = chars.peek().and_then(|(_, c)| c.to_digit(16))
        {
            code = code * 16 + digit;
            digits += 1;
            chars.next();
        }
        if digits > 0 {
            chars.next_if(|(_, c)| c.is_ascii_whitespace());
            value.push(
                char::from_u32(code)
                    .filter(|&c| c != '\0')
                    .unwrap_or('\u{fffd}'),
            );
        } else if let Some((_, c)) = chars.next()
            && c != '\n'
        {
            value.push(c);
        }
    }
    (value, source.len())
}

pub(crate) fn identifier(source: &str) -> Option<(String, usize)> {
    let (name, length) = unescape(source, |c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii())
    });
    let raw = source.as_bytes();
    let invalid = length == 0
        || raw[0].is_ascii_digit()
        || raw[0] == b'-' && (length == 1 || raw[1].is_ascii_digit());
    (!invalid).then_some((name, length))
}

// The argument of a `::view-transition-*()` pseudo-element: a name or `*`, then classes, as `name.a.b`.
fn transition_argument(argument: &str) -> Option<String> {
    let argument = argument.trim();
    let (mut out, mut rest) = match argument.strip_prefix('*') {
        Some(rest) => ("*".to_owned(), rest),
        None if argument.starts_with('.') => ("*".to_owned(), argument),
        None => {
            let (name, length) = identifier(argument)?;
            (name != "default").then_some(())?;
            (name, argument[length..].trim_start())
        }
    };
    while let Some(after) = rest.strip_prefix('.') {
        let (class, length) = identifier(after)?;
        out.push('.');
        out.push_str(&class);
        rest = &after[length..];
    }
    rest.is_empty().then_some(out)
}

pub(crate) fn selector_boundary(source: &str, separator: impl Fn(char) -> bool) -> usize {
    let mut depth = 0;
    let mut quote = None;
    let mut escape = None;
    for (index, c) in source.char_indices() {
        if let Some(digits) = escape {
            if c.is_ascii_hexdigit() && digits < 6 {
                escape = Some(digits + 1);
                continue;
            }
            escape = None;
            if digits == 0 || c.is_ascii_whitespace() {
                continue;
            }
        }
        if c == '\\' {
            escape = Some(0);
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, '\'' | '"') {
            quote = Some(c);
            continue;
        }
        if depth == 0 && separator(c) {
            return index;
        }
        if matches!(c, '[' | '(') {
            depth += 1;
        } else if matches!(c, ']' | ')') {
            depth -= 1;
        }
    }
    source.len()
}

impl Selector {
    // A shadow root is featureless, so callers must not skip matching for it with this.
    pub(crate) fn always(&self) -> bool {
        self.previous.is_none()
            && self
                .simple
                .iter()
                .all(|simple| matches!(simple, SimpleSelector::Universal))
    }
    pub(crate) fn host_only(&self) -> bool {
        self.previous.is_none()
            && self
                .simple
                .iter()
                .any(|simple| matches!(simple, SimpleSelector::Pseudo(name, ..) if name == "host"))
    }

    pub(crate) fn argument(
        name: &str,
        argument: &str,
        parent: Option<&SelectorList>,
        strict: bool,
    ) -> Option<(Vec<Self>, [i32; 2])> {
        let list = |source: &str| {
            Self::list(source, parent, strict, false).filter(|list| {
                !list.is_empty()
                    && !list.iter().any(|selector| {
                        let mut simple = selector.simple.iter();
                        simple.any(|simple| matches!(simple, SimpleSelector::Element(..)))
                    })
            })
        };
        match name {
            "is" | "where" => Some((
                split(argument)
                    .into_iter()
                    .flat_map(|part| list(part).unwrap_or_default())
                    .collect(),
                [0, 0],
            )),
            "not" => Some((list(argument)?, [0, 0])),
            "host" if !argument.is_empty() => Some((list(argument)?, [0, 0])),
            "has" => {
                let relative = split(argument)
                    .into_iter()
                    .map(|part| {
                        (!part.trim().is_empty()).then(|| format!(":scope {}", part.trim()))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let selectors = list(&relative.join(","))?;
                // Nested `:has()` is invalid and makes matching cost O(N²) or more.
                if selectors
                    .iter()
                    .any(|selector| selector.states(&mut Vec::new()) & HAS != 0)
                {
                    return None;
                }
                Some((selectors, [0, 0]))
            }
            "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                let of = argument.as_bytes().windows(4).position(|window| {
                    window[0].is_ascii_whitespace()
                        && window[1..3].eq_ignore_ascii_case(b"of")
                        && window[3].is_ascii_whitespace()
                });
                let (formula, selectors) = match of {
                    Some(_) if name.ends_with("of-type") => return None,
                    Some(index) => (&argument[..index], list(&argument[index + 4..])?),
                    None => (argument, Vec::new()),
                };
                let formula = formula.replace("/**/", " ").trim().to_ascii_lowercase();
                let nth = match formula.as_str() {
                    "odd" => [2, 1],
                    "even" => [2, 0],
                    _ => match formula.split_once('n') {
                        None => [0, formula.parse().ok()?],
                        Some((a, b)) => {
                            let b = b.trim_start();
                            let (negative, digits) = match b.as_bytes().first() {
                                None => (false, "0"),
                                Some(b'+') => (false, b[1..].trim_start()),
                                Some(b'-') => (true, b[1..].trim_start()),
                                _ => return None,
                            };
                            if !digits.starts_with(|c: char| c.is_ascii_digit()) {
                                return None;
                            }
                            let b: i32 = digits.parse().ok()?;
                            [
                                match a {
                                    "" | "+" => 1,
                                    "-" => -1,
                                    _ => a.parse().ok()?,
                                },
                                if negative { -b } else { b },
                            ]
                        }
                    },
                };
                Some((selectors, nth))
            }
            "lang" | "dir" => (!argument.trim().is_empty()).then(|| (Vec::new(), [0, 0])),
            _ => Some((Vec::new(), [0, 0])),
        }
    }

    pub(crate) fn bind_parent(&mut self, parent: Option<&SelectorList>) -> anyhow::Result<()> {
        for selector in &mut self.simple {
            match selector {
                SimpleSelector::Parent(selectors, _) => {
                    *selectors = parent
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Missing CSS parent selector"))?;
                }
                SimpleSelector::Pseudo(name, argument, parsed, _, scoped) => {
                    let outer = SCOPED.replace(*scoped);
                    let selectors = Self::argument(name, argument, parent, false);
                    SCOPED.set(outer);
                    if let Some((selectors, _)) = selectors {
                        *parsed = selectors.into();
                    }
                }
                _ => {}
            }
        }
        if let Some((_, previous)) = &mut self.previous {
            previous.bind_parent(parent)?;
        }
        Ok(())
    }

    pub(crate) fn states(&self, parents: &mut Vec<SelectorList>) -> u32 {
        let mut states = self
            .previous
            .as_ref()
            .map_or(0, |(_, previous)| previous.states(parents));
        for simple in &self.simple {
            match simple {
                SimpleSelector::Pseudo(name, _, parsed, ..) => {
                    states |= match name.as_str() {
                        "hover" => HOVER,
                        "active" => ACTIVE,
                        "focus" | "focus-visible" => FOCUS,
                        "focus-within" => FOCUS_WITHIN,
                        "has" => HAS,
                        _ => 0,
                    };
                    for selector in parsed.iter() {
                        states |= selector.states(parents);
                    }
                }
                SimpleSelector::Parent(list, _) => parents.push(list.clone()),
                _ => {}
            }
        }
        states
    }

    pub(crate) fn without_pseudo_element(&self) -> Option<(usize, Self)> {
        let (index, kind) = self.simple.iter().enumerate().find_map(|(index, part)| {
            let SimpleSelector::Element(name, _) = part else {
                return None;
            };
            Some((
                index,
                [
                    "before",
                    "after",
                    "selection",
                    "marker",
                    "first-letter",
                    "first-line",
                    "placeholder",
                    "-webkit-scrollbar",
                    "-webkit-scrollbar-track",
                    "-webkit-scrollbar-thumb",
                    "-webkit-scrollbar-corner",
                    "-webkit-scrollbar-track-piece",
                    "slotted",
                    "part",
                    "starting-style",
                    "backdrop",
                    "-webkit-slider-runnable-track",
                    "-webkit-slider-thumb",
                    "-webkit-progress-bar",
                    "-webkit-progress-value",
                    "file-selector-button",
                    "-webkit-file-upload-button",
                    "highlight",
                    "view-transition",
                    "view-transition-group",
                    "view-transition-image-pair",
                    "view-transition-old",
                    "view-transition-new",
                ]
                .iter()
                .position(|n| n == name)
                // The parts of ranges, progress bars and file inputs share the slots of the scrollbar track and
                // thumb.
                .map(|kind| match kind {
                    16 | 18 | 20 | 21 => 8,
                    17 | 19 => 9,
                    kind => kind,
                })?,
            ))
        })?;
        let mut selector = self.clone();
        if kind == 15
            && matches!(&self.simple[index + 1..], [SimpleSelector::Element(name, _)]
                if name == "starting-style")
        {
            selector.simple.truncate(index);
            return Some((16, selector));
        }
        if matches!(kind, 12 | 13) {
            selector.simple.truncate(index);
            return Some((kind, selector));
        }
        if !(7..12).contains(&kind) {
            selector.simple.remove(index);
            return Some((kind, selector));
        }
        let kind = match (kind, &self.simple[index + 1..]) {
            (11, []) => 8,
            (_, []) => kind,
            (9, [SimpleSelector::Pseudo(name, ..)]) if name == "hover" => 11,
            _ => return None,
        };
        selector.simple.truncate(index);
        Some((kind, selector))
    }

    pub(crate) fn shadow_target(&self) -> Option<(usize, Self)> {
        let index = self
            .simple
            .iter()
            .position(|part| matches!(part, SimpleSelector::Element(..)))?;
        let part =
            matches!(&self.simple[index], SimpleSelector::Element(name, _) if name == "part");
        let rest = &self.simple[index + 1..];
        let split = rest
            .iter()
            .position(|part| matches!(part, SimpleSelector::Element(..)))
            .unwrap_or(rest.len());
        let compound = |simple: &[SimpleSelector]| Self {
            simple: simple.to_vec(),
            previous: None,
            specificity: [0; 3],
            ancestry: Default::default(),
        };
        let kind = match compound(&rest[split..]).without_pseudo_element() {
            _ if split == rest.len() => usize::MAX,
            Some((kind, left)) if left.simple.is_empty() => kind,
            _ => return None,
        };
        let classes = compound(&rest[..split]);
        let allowed = if part {
            !matches!(kind, 12 | 13 | 22) && classes.stateful()
        } else {
            matches!(kind, 0 | 1 | 3 | 6 | 14..=16 | usize::MAX) && classes.simple.is_empty()
        };
        allowed.then_some((kind, classes))
    }

    pub(crate) fn stateful(&self) -> bool {
        self.previous.is_none()
            && self.simple.iter().all(|simple| {
                matches!(simple, SimpleSelector::Pseudo(name, _, parsed, ..)
                    if !(name.ends_with("child")
                        || name.ends_with("of-type")
                        || matches!(name.as_str(), "empty" | "has" | "host" | "host-context" | "scope"))
                        && parsed.iter().all(Self::stateful))
            })
    }

    pub(crate) fn pseudo_argument(&self) -> Option<&str> {
        self.simple.iter().find_map(|part| match part {
            SimpleSelector::Element(_, argument) => Some(argument.as_str()),
            _ => None,
        })
    }

    // Match matches_with's ancestor dependencies: classes (8), otherwise tag and attributes (0), plus style
    // (6).
    pub(crate) fn ancestry(&self) -> Option<&Ancestry> {
        self.ancestry
            .get_or_init(|| {
                let (mut bloom, mut steps, mut rest) = ([0; BLOOM], Vec::new(), 0u16);
                let mut link = &self.previous;
                while let Some((combinator, previous)) = link {
                    if !matches!(combinator, ' ' | '>') {
                        return None;
                    }
                    let mut kinds = 1 << 1;
                    let mut classes = false;
                    for simple in &previous.simple {
                        match simple {
                            SimpleSelector::Tag(name) => feature(&mut bloom, 0, &lowercase(name)),
                            SimpleSelector::Id(id) => feature(&mut bloom, 1, id),
                            SimpleSelector::Class(class) => {
                                classes = true;
                                feature(&mut bloom, 2, class);
                            }
                            SimpleSelector::Attribute(name, ..) => {
                                if name.eq_ignore_ascii_case("style") {
                                    kinds |= 1 << 6;
                                }
                                feature(&mut bloom, 3, &lowercase(name));
                            }
                            SimpleSelector::Universal => {}
                            _ => return None,
                        }
                    }
                    if !classes
                        || previous.simple.iter().any(|simple| {
                            !matches!(simple, SimpleSelector::Class(_) | SimpleSelector::Universal)
                        })
                    {
                        kinds |= 1;
                    }
                    if classes {
                        kinds |= 1 << 8;
                    }
                    if *combinator == '>' && rest == 0 {
                        steps.push(kinds);
                    } else {
                        rest |= kinds;
                    }
                    link = &previous.previous;
                }
                (bloom != [0; BLOOM]).then(|| (bloom, steps.into(), rest))
            })
            .as_ref()
    }

    // A node can match only if it has one of these keys. Empty means any node can match. Kind 4 is an exact
    // attribute value, keyed as `name=value`.
    pub(crate) fn keys(&self) -> Vec<(u8, Cow<'_, str>)> {
        if let Some(key) = self.own_key() {
            return vec![key];
        }
        for part in &self.simple {
            let alternatives = match part {
                SimpleSelector::Parent(selectors, _) => selectors.as_slice(),
                SimpleSelector::Pseudo(name, _, selectors, ..)
                    if matches!(name.as_str(), "is" | "where") =>
                {
                    &selectors[..]
                }
                _ => continue,
            };
            let mut keys: Vec<(u8, Cow<str>)> = Vec::new();
            for selector in alternatives {
                let more = selector.keys();
                if more.is_empty() {
                    keys.clear();
                    break;
                }
                for key in more {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            if !keys.is_empty() && keys.len() <= 32 {
                return keys;
            }
        }
        Vec::new()
    }

    pub(crate) fn own_key(&self) -> Option<(u8, Cow<'_, str>)> {
        self.simple
            .iter()
            .filter_map(|selector| match selector {
                SimpleSelector::Id(id) => Some((0, Cow::Borrowed(id.as_str()))),
                SimpleSelector::Class(class) => Some((1, Cow::Borrowed(class.as_str()))),
                // Keys are lowercase, and nodes look up their names in lowercase, so mixed-case names get no key.
                SimpleSelector::Tag(name) if !name.bytes().any(|c| c.is_ascii_uppercase()) => {
                    Some((2, Cow::Borrowed(name.as_str())))
                }
                SimpleSelector::Attribute(name, operator, value, 0)
                    if operator == "=" && !name.bytes().any(|c| c.is_ascii_uppercase()) =>
                {
                    Some((4, Cow::Owned(format!("{name}={value}"))))
                }
                SimpleSelector::Attribute(name, ..)
                    if !name.bytes().any(|c| c.is_ascii_uppercase()) =>
                {
                    Some((3, Cow::Borrowed(name.as_str())))
                }
                _ => None,
            })
            .min_by_key(|(kind, _)| [0, 1, 3, 4, 2][*kind as usize])
    }

    pub(crate) fn source(&self) -> String {
        let escape = |source: &mut String, value: &str| {
            for (index, c) in value.char_indices() {
                if c.is_ascii_control()
                    || c == ' '
                    || c.is_ascii_digit() && (index == 0 || index == 1 && value.starts_with('-'))
                {
                    source.push_str(&format!("\\{:x} ", c as u32));
                } else {
                    if !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii()) {
                        source.push('\\');
                    }
                    source.push(c);
                }
            }
        };
        let mut source = String::new();
        if let Some((combinator, previous)) = &self.previous {
            source.push_str(&previous.source());
            source.push(' ');
            if *combinator != ' ' {
                source.push(*combinator);
                source.push(' ');
            }
        }
        for selector in &self.simple {
            match selector {
                SimpleSelector::Tag(tag) => escape(&mut source, tag),
                SimpleSelector::Id(id) => {
                    source.push('#');
                    escape(&mut source, id);
                }
                SimpleSelector::Class(class) => {
                    source.push('.');
                    escape(&mut source, class);
                }
                SimpleSelector::Universal => source.push('*'),
                SimpleSelector::Root => source.push_str(":root"),
                SimpleSelector::Attribute(name, op, value, insensitive) => {
                    source.push('[');
                    escape(&mut source, name);
                    source.push_str(op);
                    if !op.is_empty() {
                        source.push('"');
                        source.push_str(
                            &value
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                                .replace('\n', "\\a "),
                        );
                        source.push('"');
                    }
                    if *insensitive == 1 {
                        source.push_str(" i");
                    }
                    source.push(']');
                }
                SimpleSelector::Pseudo(name, argument, ..)
                | SimpleSelector::Element(name, argument) => {
                    source.push(':');
                    if matches!(selector, SimpleSelector::Element(..)) {
                        source.push(':');
                    }
                    source.push_str(name);
                    if !argument.is_empty() {
                        source.push('(');
                        source.push_str(argument);
                        source.push(')');
                    }
                }
                SimpleSelector::Parent(_, 0) => source.push_str(":scope"),
                SimpleSelector::Parent(_, count) => source.extend(std::iter::repeat_n('&', *count)),
                SimpleSelector::NoNamespace => source.push('|'),
            }
        }
        source
    }

    pub(crate) fn parse(source: &str, parent: Option<&SelectorList>) -> Vec<Self> {
        Self::list(source, parent, true, true).unwrap_or_default()
    }

    // Rules of an `@scope` rule, where `:scope` is the root of the scope.
    pub(crate) fn parse_scoped(source: &str, parent: Option<&SelectorList>) -> Vec<Self> {
        SCOPED.set(true);
        let selectors = Self::parse(source, parent);
        SCOPED.set(false);
        selectors
    }

    pub(crate) fn try_parse(source: &str) -> anyhow::Result<Vec<Self>> {
        Self::list(source, None, true, true)
            .ok_or_else(|| anyhow::anyhow!("SyntaxError: '{source}' is not a valid selector"))
    }

    pub(crate) fn list(
        source: &str,
        parent: Option<&SelectorList>,
        strict: bool,
        nested: bool,
    ) -> Option<Vec<Self>> {
        let mut depth = 0u32;
        if source.bytes().any(|c| {
            depth = match c {
                b'(' => depth + 1,
                b')' => depth.saturating_sub(1),
                _ => depth,
            };
            depth > 32
        }) {
            return Some(Vec::new());
        }
        let mut selectors = Vec::new();
        let parent_specificity = parent
            .and_then(|p| p.iter().map(|s| s.specificity).max())
            .unwrap_or_default();
        for source in split(source) {
            let mut rest = source.trim();
            if rest.is_empty() {
                return None;
            }
            let (mut quote, mut escaped) = (None, false);
            let scoped = SCOPED.get();
            let explicit = rest.char_indices().any(|(at, c)| {
                let plain = !escaped && quote.is_none();
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if quote == Some(c) {
                    quote = None;
                } else if plain && matches!(c, '"' | '\'') {
                    quote = Some(c);
                }
                plain && (c == '&' || scoped && rest[at..].starts_with(":scope"))
            });
            let mut previous = parent
                .filter(|_| nested && (!explicit || rest.starts_with(['>', '+', '~'])))
                .map(|parent| Self {
                    simple: vec![SimpleSelector::Parent(Rc::clone(parent), 1)],
                    previous: None,
                    specificity: parent_specificity,
                    ancestry: Default::default(),
                });
            let mut compounds = 0;
            while !rest.is_empty() {
                // Matching recurses per compound, so long chains can overflow the stack.
                compounds += 1;
                if compounds > 256 {
                    return Some(Vec::new());
                }
                let mut combinator = ' ';
                let before = previous
                    .as_ref()
                    .map_or(&[][..], |previous| &previous.simple);
                if before
                    .iter()
                    .any(|simple| matches!(simple, SimpleSelector::Element(..)))
                {
                    return None;
                }
                if rest.starts_with(['>', '+', '~']) {
                    previous.as_ref()?;
                    combinator = rest.as_bytes()[0] as char;
                    rest = rest[1..].trim_start();
                }
                let end = selector_boundary(rest, |c| {
                    c.is_ascii_whitespace() || matches!(c, '>' | '+' | '~')
                });
                if end == 0 {
                    return None;
                }
                let mut compound = &rest[..end];
                let none = compound.starts_with('|');
                if let Some(tail) = compound
                    .strip_prefix("*|")
                    .or_else(|| compound.strip_prefix('|'))
                {
                    if !tail.starts_with('*') && identifier(tail).is_none() {
                        return None;
                    }
                    compound = tail;
                } else if compound.contains('|')
                    && let Some((_, length)) = identifier(compound)
                    && compound[length..].starts_with('|')
                {
                    return None;
                }
                let mut simple = Vec::new();
                let mut parent_index = None;
                let mut specificity = previous.as_ref().map_or([0; 3], |s| s.specificity);
                while !compound.is_empty() {
                    let marker = compound.as_bytes()[0];
                    if marker == b'[' {
                        let end = 1 + selector_boundary(&compound[1..], |c| c == ']');
                        if end == compound.len() {
                            return None;
                        }
                        let attribute = compound[1..end].trim();
                        let attribute = attribute
                            .strip_prefix("*|")
                            .or_else(|| {
                                attribute
                                    .strip_prefix('|')
                                    .filter(|rest| !rest.starts_with('='))
                            })
                            .unwrap_or(attribute);
                        let (name, length) = identifier(attribute)?;
                        let tail = attribute[length..].trim_start();
                        let (operator, value, insensitive) = if tail.is_empty() {
                            ("", String::new(), 0)
                        } else {
                            let operator = ["=", "~=", "|=", "^=", "$=", "*="]
                                .into_iter()
                                .find(|operator| tail.starts_with(operator))?;
                            let tail = tail[operator.len()..].trim_start();
                            let (value, length) = if let Some(quote) =
                                tail.chars().next().filter(|c| matches!(c, '"' | '\''))
                            {
                                let (value, length) = unescape(&tail[1..], |c| c == quote);
                                if length + 1 == tail.len() {
                                    return None;
                                }
                                (value, length + 2)
                            } else {
                                identifier(tail)?
                            };
                            let insensitive = match tail[length..].trim_start() {
                                "" if HTML_INSENSITIVE
                                    .split(' ')
                                    .any(|known| name.eq_ignore_ascii_case(known)) =>
                                {
                                    2
                                }
                                "" | "s" | "S" => 0,
                                "i" | "I" => 1,
                                _ => return None,
                            };
                            (operator, value, insensitive)
                        };
                        simple.push(SimpleSelector::Attribute(
                            name,
                            operator.into(),
                            value,
                            insensitive,
                        ));
                        specificity[1] = specificity[1].saturating_add(1);
                        compound = &compound[end + 1..];
                        continue;
                    }
                    if matches!(marker, b'&' | b'*') {
                        if marker == b'*' && !simple.is_empty() {
                            return None;
                        }
                        if marker == b'&' {
                            if let Some(parent) = parent {
                                if let Some(index) = parent_index {
                                    if let SimpleSelector::Parent(_, count) = &mut simple[index] {
                                        *count += 1;
                                    }
                                } else {
                                    parent_index = Some(simple.len());
                                    simple.push(SimpleSelector::Parent(Rc::clone(parent), 1));
                                }
                                for (value, inherited) in
                                    specificity.iter_mut().zip(parent_specificity)
                                {
                                    *value = value.saturating_add(inherited);
                                }
                            } else {
                                simple.push(SimpleSelector::Pseudo(
                                    "scope".into(),
                                    String::new(),
                                    Rc::new([]),
                                    [0, 0],
                                    false,
                                ));
                            }
                        } else {
                            simple.push(SimpleSelector::Universal);
                        }
                        compound = &compound[1..];
                        continue;
                    }
                    if marker == b':' {
                        // Like Chrome, nothing can follow a view transition pseudo-element.
                        if simple.iter().any(|part| {
                            matches!(part, SimpleSelector::Element(name, _) if name.starts_with("view-transition"))
                        }) {
                            return None;
                        }
                        let element = compound.starts_with("::");
                        compound = &compound[if element { 2 } else { 1 }..];
                        let (name, length) = identifier(compound)?;
                        let name = name.to_ascii_lowercase();
                        compound = &compound[length..];
                        let mut argument = "";
                        let functional = compound.starts_with('(');
                        if functional {
                            let close = 1 + selector_boundary(&compound[1..], |c| c == ')');
                            if close == compound.len() {
                                return None;
                            }
                            argument = &compound[1..close];
                            compound = &compound[close + 1..];
                        }
                        let element = element
                            || matches!(
                                name.as_str(),
                                "before" | "after" | "first-line" | "first-letter"
                            );
                        if strict
                            && !PSEUDOS[usize::from(element) * 2 + usize::from(functional)]
                                .split(' ')
                                .any(|known| known == name)
                            && !(element && !functional && name.starts_with("-webkit-"))
                            && name != "-internal-dialog-in-top-layer"
                        {
                            return None;
                        }
                        let ident;
                        if functional && (name == "state" || element && name == "highlight") {
                            let length;
                            argument = argument.trim();
                            (ident, length) = identifier(argument)?;
                            if length != argument.len() {
                                return None;
                            }
                            argument = &ident;
                        }
                        if name == "active-view-transition-type"
                            && !argument.split(',').all(|kind| {
                                let kind = kind.trim();
                                identifier(kind).is_some_and(|(_, length)| length == kind.len())
                            })
                        {
                            return None;
                        }
                        let named;
                        if element && functional && name.starts_with("view-transition-") {
                            named = transition_argument(argument)?;
                            if !named.starts_with(['*', '.']) {
                                specificity[2] = specificity[2].saturating_add(1);
                            }
                            argument = &named;
                        }
                        if element {
                            specificity[2] = specificity[2].saturating_add(1);
                            simple.push(SimpleSelector::Element(name, argument.into()));
                            continue;
                        }
                        if name == "root" {
                            specificity[1] = specificity[1].saturating_add(1);
                            simple.push(SimpleSelector::Root);
                            continue;
                        }
                        // In an `@scope` rule, `:scope` is its root, with the specificity of a pseudo-class.
                        if name == "scope"
                            && !functional
                            && let Some(parent) = parent.filter(|_| SCOPED.get())
                        {
                            if parent_index.is_none() {
                                parent_index = Some(simple.len());
                                simple.push(SimpleSelector::Parent(Rc::clone(parent), 0));
                            }
                            specificity[1] = specificity[1].saturating_add(1);
                            continue;
                        }
                        let (selectors, nth) = Self::argument(&name, argument, parent, strict)?;
                        let [ids, classes, tags] = selectors
                            .iter()
                            .map(|selector| selector.specificity)
                            .max()
                            .unwrap_or_default();
                        let own = match name.as_str() {
                            "where" => [0; 3],
                            "is" | "not" => [ids, classes, tags],
                            "has" => [ids, classes.saturating_sub(1), tags],
                            _ => [ids, classes.saturating_add(1), tags],
                        };
                        for (value, own) in specificity.iter_mut().zip(own) {
                            *value = value.saturating_add(own);
                        }
                        simple.push(SimpleSelector::Pseudo(
                            name,
                            argument.into(),
                            selectors.into(),
                            nth,
                            SCOPED.get(),
                        ));
                        continue;
                    }
                    if matches!(marker, b'.' | b'#') {
                        compound = &compound[1..];
                    } else if !simple.is_empty() {
                        return None;
                    }
                    let (name, length) = identifier(compound)?;
                    compound = &compound[length..];
                    simple.push(match marker {
                        b'#' => {
                            specificity[0] = specificity[0].saturating_add(1);
                            SimpleSelector::Id(name)
                        }
                        b'.' => {
                            specificity[1] = specificity[1].saturating_add(1);
                            SimpleSelector::Class(name)
                        }
                        _ => {
                            specificity[2] = specificity[2].saturating_add(1);
                            SimpleSelector::Tag(name)
                        }
                    });
                }
                if none {
                    simple.insert(0, SimpleSelector::NoNamespace);
                }
                previous = Some(Self {
                    simple,
                    previous: previous.map(|s| (combinator, Box::new(s))),
                    specificity,
                    ancestry: Default::default(),
                });
                rest = rest[end..].trim_start();
            }
            selectors.push(previous?);
        }
        Some(selectors)
    }
}
