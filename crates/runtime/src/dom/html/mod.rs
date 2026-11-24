use super::document::Attrs;
use super::{Document, Node};

mod entities;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Initial,
    BeforeHead,
    Head,
    AfterHead,
    Body,
    Table,
    Caption,
    Columns,
    Section,
    Row,
    Cell,
    Select,
    Template,
}

const DEFAULT: usize = 0;
const LIST: usize = 1;
const BUTTON: usize = 2;
const TABLE: usize = 3;
const SELECT: usize = 4;
const SPECIAL: usize = 5;
const HTML: usize = 6;
const MARKER: usize = 7;

pub(super) const RAW: [&str; 8] = [
    "style",
    "script",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
];
const IMPLIED: [&str; 10] = [
    "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc",
];

const SVG_TAGS: &str = "altGlyph altGlyphDef altGlyphItem animateColor animateMotion animateTransform clipPath feBlend feColorMatrix feComponentTransfer feComposite feConvolveMatrix feDiffuseLighting feDisplacementMap feDistantLight feDropShadow feFlood feFuncA feFuncB feFuncG feFuncR feGaussianBlur feImage feMerge feMergeNode feMorphology feOffset fePointLight feSpecularLighting feSpotLight feTile feTurbulence foreignObject glyphRef linearGradient radialGradient textPath";
const SVG_ATTRS: &str = "attributeName attributeType baseFrequency baseProfile calcMode clipPathUnits diffuseConstant edgeMode filterUnits glyphRef gradientTransform gradientUnits kernelMatrix kernelUnitLength keyPoints keySplines keyTimes lengthAdjust limitingConeAngle markerHeight markerUnits markerWidth maskContentUnits maskUnits numOctaves pathLength patternContentUnits patternTransform patternUnits pointsAtX pointsAtY pointsAtZ preserveAlpha preserveAspectRatio primitiveUnits refX refY repeatCount repeatDur requiredExtensions requiredFeatures specularConstant specularExponent spreadMethod startOffset stdDeviation stitchTiles surfaceScale systemLanguage tableValues targetX targetY textLength viewBox viewTarget xChannelSelector yChannelSelector zoomAndPan";

struct Open {
    id: usize,
    key: u64,
    svg: bool,
    bits: u8,
    prev: usize,
    below: [usize; 7],
    saved: Option<Mode>,
}

enum Content {
    Markup,
    Raw(usize, bool),
    Plain,
}

struct Builder {
    nodes: Vec<Node>,
    stack: Vec<Open>,
    active: Vec<Option<(usize, Attrs)>>,
    opened: Vec<bool>,
    last: rustc_hash::FxHashMap<u64, usize>,
    bound: [usize; 7],
    mode: Mode,
    head: Option<usize>,
    form: Option<usize>,
    foster: bool,
    newline: bool,
}

fn key(tag: &str) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    for byte in tag.bytes() {
        std::hash::Hasher::write_u8(&mut hasher, byte.to_ascii_lowercase());
    }
    std::hash::Hasher::finish(&hasher)
}

fn tag(source: &str) -> Option<(String, Attrs, bool, usize)> {
    let bytes = source.as_bytes();
    let space = |c: u8| matches!(c, b'\t' | b'\n' | b'\x0c' | b' ');
    let mut i = bytes
        .iter()
        .position(|&c| space(c) || c == b'/' || c == b'>')?;
    let name = source[..i].to_ascii_lowercase();
    let mut attrs = Attrs::default();
    let mut names: Option<rustc_hash::FxHashSet<String>> = None;
    let mut closing = false;
    loop {
        match *bytes.get(i)? {
            b'>' => return Some((name, attrs, closing, i + 1)),
            b'/' => {
                i += 1;
                closing = bytes.get(i) == Some(&b'>');
                continue;
            }
            c if space(c) => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let start = i;
        i += 1;
        while bytes
            .get(i)
            .is_some_and(|&c| !space(c) && !matches!(c, b'/' | b'>' | b'='))
        {
            i += 1;
        }
        let key = source[start..i].to_ascii_lowercase();
        while bytes.get(i).copied().is_some_and(space) {
            i += 1;
        }
        let mut value = String::new();
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).copied().is_some_and(space) {
                i += 1;
            }
            if let Some(&quote @ (b'"' | b'\'')) = bytes.get(i) {
                let end = i + 1 + source[i + 1..].find(quote as char)?;
                value = decode(&source[i + 1..end], true);
                i = end + 1;
            } else {
                let start = i;
                while bytes.get(i).is_some_and(|&c| !space(c) && c != b'>') {
                    i += 1;
                }
                value = decode(&source[start..i], true);
            }
        }
        let new = match &mut names {
            Some(names) => names.insert(key.clone()),
            None if attrs.len() < 32 => !attrs.contains_key(&key),
            None => {
                let mut set: rustc_hash::FxHashSet<String> = attrs.keys().cloned().collect();
                let new = set.insert(key.clone());
                names = Some(set);
                new
            }
        };
        if new {
            attrs.append(key, value);
        }
        closing = false;
    }
}

impl Builder {
    fn push(&mut self, id: usize, svg: bool, mode: Option<Mode>) {
        let tag = self.nodes[id].tag.as_str();
        let html = !svg;
        let point = svg && matches!(tag, "foreignObject" | "desc" | "title");
        let default = point
            || html
                && matches!(
                    tag,
                    "applet"
                        | "caption"
                        | "html"
                        | "table"
                        | "td"
                        | "th"
                        | "marquee"
                        | "object"
                        | "template"
                );
        let special = point
            || html
                && matches!(
                    tag,
                    "address"
                        | "applet"
                        | "area"
                        | "article"
                        | "aside"
                        | "base"
                        | "basefont"
                        | "bgsound"
                        | "blockquote"
                        | "body"
                        | "br"
                        | "button"
                        | "caption"
                        | "center"
                        | "col"
                        | "colgroup"
                        | "dd"
                        | "details"
                        | "dir"
                        | "div"
                        | "dl"
                        | "dt"
                        | "embed"
                        | "fieldset"
                        | "figcaption"
                        | "figure"
                        | "footer"
                        | "form"
                        | "frame"
                        | "frameset"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "head"
                        | "header"
                        | "hgroup"
                        | "hr"
                        | "html"
                        | "iframe"
                        | "img"
                        | "input"
                        | "keygen"
                        | "li"
                        | "link"
                        | "listing"
                        | "main"
                        | "marquee"
                        | "menu"
                        | "meta"
                        | "nav"
                        | "noembed"
                        | "noframes"
                        | "noscript"
                        | "object"
                        | "ol"
                        | "p"
                        | "param"
                        | "plaintext"
                        | "pre"
                        | "script"
                        | "search"
                        | "section"
                        | "select"
                        | "source"
                        | "style"
                        | "summary"
                        | "table"
                        | "tbody"
                        | "td"
                        | "template"
                        | "textarea"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "title"
                        | "tr"
                        | "track"
                        | "ul"
                        | "wbr"
                        | "xmp"
                );
        let marker = html
            && matches!(
                tag,
                "applet" | "caption" | "marquee" | "object" | "td" | "th" | "template"
            );
        let bits = u8::from(default)
            | u8::from(default || html && matches!(tag, "ol" | "ul")) << LIST
            | u8::from(default || html && tag == "button") << BUTTON
            | u8::from(html && matches!(tag, "html" | "table" | "template")) << TABLE
            | u8::from(!(html && matches!(tag, "optgroup" | "option"))) << SELECT
            | u8::from(special) << SPECIAL
            | u8::from(html) << HTML
            | u8::from(marker) << MARKER;
        let key = key(tag);
        let saved = mode.map(|mode| std::mem::replace(&mut self.mode, mode));
        if marker {
            self.active.push(None);
        }
        if self.opened.len() <= id {
            self.opened.resize(id + 1, false);
        }
        self.enter(Open {
            id,
            key,
            svg,
            bits,
            prev: 0,
            below: [0; 7],
            saved,
        });
    }

    fn enter(&mut self, mut open: Open) {
        let at = self.stack.len() + 1;
        open.prev = self.last.insert(open.key, at).unwrap_or(0);
        open.below = self.bound;
        for (kind, bound) in self.bound.iter_mut().enumerate() {
            if open.bits & 1 << kind != 0 {
                *bound = at;
            }
        }
        self.opened[open.id] = true;
        self.stack.push(open);
    }

    fn leave(&mut self, open: &Open) {
        match open.prev {
            0 => self.last.remove(&open.key),
            prev => self.last.insert(open.key, prev),
        };
        self.bound = open.below;
    }

    fn pop(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(open) = self.stack.pop() else { return };
        self.opened[open.id] = false;
        if open.bits & 1 << MARKER != 0 {
            while let Some(Some(_)) = self.active.pop() {}
        }
        self.leave(&open);
        if let Some(mode) = open.saved {
            self.mode = mode;
        }
    }

    fn pop_to(&mut self, position: usize) {
        while position > 1 && self.stack.len() >= position {
            self.pop();
        }
    }

    fn split(&mut self, at: usize) -> Vec<Open> {
        let tail = self.stack.split_off(at);
        for open in tail.iter().rev() {
            self.leave(open);
        }
        tail
    }

    fn restore(&mut self, tail: Vec<Open>) {
        for open in tail {
            self.enter(open);
        }
    }

    // Only for elements that are not formatting markers and saved no mode.
    fn remove(&mut self, at: usize) {
        let mut tail = self.split(at);
        self.opened[tail.remove(0).id] = false;
        self.restore(tail);
    }

    fn scope(&self, tag: &str, kind: usize) -> usize {
        self.last
            .get(&key(tag))
            .copied()
            .filter(|&at| at >= self.bound[kind])
            .unwrap_or(0)
    }

    fn is(&self, tag: &str) -> bool {
        self.stack
            .last()
            .is_some_and(|open| !open.svg && self.nodes[open.id].tag == tag)
    }

    fn foreign(&self) -> bool {
        self.stack.last().is_some_and(|open| {
            open.svg
                && !matches!(
                    self.nodes[open.id].tag.as_str(),
                    "foreignObject" | "desc" | "title"
                )
        })
    }

    fn clear(&mut self, tags: &[&str]) {
        while self.stack.len() > 1
            && !self
                .stack
                .last()
                .is_some_and(|open| !open.svg && tags.contains(&self.nodes[open.id].tag.as_str()))
        {
            self.pop();
        }
    }

    fn place(&self, target: Option<&Open>) -> (usize, Option<usize>) {
        let svg = target.is_some_and(|open| open.svg);
        let target = target.map_or(0, |open| open.id);
        if self.foster
            && !svg
            && matches!(
                self.nodes[target].tag.as_str(),
                "table" | "tbody" | "tfoot" | "thead" | "tr"
            )
        {
            let table = self.last.get(&key("table")).copied().unwrap_or(0);
            let template = self.last.get(&key("template")).copied().unwrap_or(0);
            if table > template && table > 1 {
                let id = self.stack[table - 1].id;
                return match self.nodes[id].parent {
                    Some(parent) => (
                        parent,
                        self.nodes[parent]
                            .children
                            .iter()
                            .rposition(|&child| child == id),
                    ),
                    None => (self.stack[table - 2].id, None),
                };
            }
            if template > 0 {
                return (self.stack[template - 1].id, None);
            }
        }
        (target, None)
    }

    fn attach(&mut self, id: usize, (parent, before): (usize, Option<usize>)) {
        self.nodes[id].parent = Some(parent);
        let children = &mut self.nodes[parent].children;
        match before {
            Some(index) => children.insert(index, id),
            None => children.push(id),
        }
    }

    fn element(&mut self, tag: &str, attrs: Attrs) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            tag: tag.into(),
            attrs,
            ..Node::default()
        });
        self.attach(id, self.place(self.stack.last()));
        id
    }

    fn open(&mut self, tag: &str, attrs: Attrs, mode: Option<Mode>) -> Content {
        let id = self.element(tag, attrs);
        self.push(id, false, mode);
        Content::Markup
    }

    fn svg(&mut self, tag: &str, attrs: Attrs, closing: bool) -> Content {
        let tag = SVG_TAGS
            .split(' ')
            .find(|name| name.eq_ignore_ascii_case(tag))
            .unwrap_or(tag);
        let attrs: Attrs = attrs
            .into_iter()
            .map(|(name, value)| {
                (
                    SVG_ATTRS
                        .split(' ')
                        .find(|known| known.eq_ignore_ascii_case(&name))
                        .map_or(name, str::to_owned),
                    value,
                )
            })
            .collect();
        let id = self.element(tag, attrs);
        if !closing {
            self.push(id, true, None);
        }
        Content::Markup
    }

    fn reconstruct(&mut self) {
        let Some(Some((last, _))) = self.active.last() else {
            return;
        };
        if self.opened[*last] {
            return;
        }
        let mut start = self.active.len() - 1;
        while start > 0 && matches!(&self.active[start - 1], Some((id, _)) if !self.opened[*id]) {
            start -= 1;
        }
        for index in start..self.active.len() {
            let Some((id, attrs)) = self.active[index].clone() else {
                continue;
            };
            let tag = self.nodes[id].tag.clone();
            let clone = self.element(&tag, attrs.clone());
            self.push(clone, false, None);
            self.active[index] = Some((clone, attrs));
        }
    }

    fn adopt(&mut self, child: usize, parent: usize) {
        if let Some(old) = self.nodes[child].parent {
            self.nodes[old].children.retain(|&id| id != child);
        }
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    fn adoption(&mut self, subject: &str) {
        let is = |builder: &Self, id: usize| builder.nodes[id].tag == subject;
        if let Some(open) = self.stack.last()
            && !open.svg
            && is(self, open.id)
            && !self.active.iter().flatten().any(|(id, _)| *id == open.id)
        {
            return self.pop();
        }
        for _ in 0..8 {
            let Some(index) = self
                .active
                .iter()
                .rposition(|entry| entry.as_ref().is_none_or(|(id, _)| is(self, *id)))
                .filter(|&index| self.active[index].is_some())
            else {
                let at = self
                    .last
                    .get(&key(subject))
                    .copied()
                    .filter(|&at| at >= self.bound[SPECIAL])
                    .unwrap_or(0);
                return self.pop_to(at);
            };
            let Some((formatting, attrs)) = self.active[index].clone() else {
                return;
            };
            let Some(position) = self.stack.iter().rposition(|open| open.id == formatting) else {
                self.active.remove(index);
                return;
            };
            if position + 1 < self.bound[DEFAULT] {
                return;
            }
            let Some(furthest) = (position + 1..self.stack.len())
                .find(|&at| self.stack[at].bits & 1 << SPECIAL != 0)
                .map(|at| self.stack[at].id)
            else {
                self.pop_to(position + 1);
                self.active.remove(index);
                return;
            };
            let mut tail = self.split(position);
            let mut bookmark = index;
            let mut last = furthest;
            let mut at = tail
                .iter()
                .position(|open| open.id == furthest)
                .unwrap_or(0);
            let mut count = 0;
            loop {
                count += 1;
                at -= 1;
                let node = tail[at].id;
                if node == formatting {
                    break;
                }
                let mut entry = self
                    .active
                    .iter()
                    .position(|entry| matches!(entry, Some((id, _)) if *id == node));
                if count > 3
                    && let Some(found) = entry.take()
                {
                    self.active.remove(found);
                    if found < bookmark {
                        bookmark -= 1;
                    }
                }
                let Some(entry) = entry else {
                    self.opened[node] = false;
                    tail.remove(at);
                    continue;
                };
                let attrs = self.active[entry]
                    .as_ref()
                    .map(|(_, attrs)| attrs.clone())
                    .unwrap_or_default();
                let clone = self.nodes.len();
                self.nodes.push(Node {
                    tag: self.nodes[node].tag.clone(),
                    attrs: attrs.clone(),
                    ..Node::default()
                });
                self.opened.resize(clone + 1, false);
                self.opened[node] = false;
                self.opened[clone] = true;
                self.active[entry] = Some((clone, attrs));
                tail[at].id = clone;
                if last == furthest {
                    bookmark = entry + 1;
                }
                self.adopt(last, clone);
                last = clone;
            }
            if let Some(old) = self.nodes[last].parent {
                self.nodes[old].children.retain(|&id| id != last);
            }
            self.attach(last, self.place(self.stack.last()));
            let clone = self.nodes.len();
            self.nodes.push(Node {
                tag: subject.into(),
                attrs: attrs.clone(),
                ..Node::default()
            });
            self.opened.resize(clone + 1, false);
            for child in std::mem::take(&mut self.nodes[furthest].children) {
                self.nodes[child].parent = Some(clone);
                self.nodes[clone].children.push(child);
            }
            self.adopt(clone, furthest);
            if let Some(old) = self
                .active
                .iter()
                .position(|entry| matches!(entry, Some((id, _)) if *id == formatting))
            {
                self.active.remove(old);
                if old < bookmark {
                    bookmark -= 1;
                }
            }
            self.active
                .insert(bookmark.min(self.active.len()), Some((clone, attrs)));
            let mut open = tail.remove(0);
            self.opened[formatting] = false;
            open.id = clone;
            let below = tail
                .iter()
                .position(|open| open.id == furthest)
                .map_or(tail.len(), |at| at + 1);
            tail.insert(below, open);
            self.restore(tail);
        }
    }

    fn insert(&mut self, text: &str) {
        let (parent, before) = self.place(self.stack.last());
        let children = &self.nodes[parent].children;
        let previous = match before {
            Some(index) => index.checked_sub(1).map(|index| children[index]),
            None => children.last().copied(),
        };
        if let Some(previous) = previous.filter(|&previous| self.nodes[previous].tag == "#text") {
            return self.nodes[previous].text.push_str(text);
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            tag: "#text".into(),
            text: text.into(),
            ..Node::default()
        });
        self.attach(id, (parent, before));
    }

    fn comment(&mut self, text: &str) {
        self.newline = false;
        let parent = self.stack.last().map_or(0, |open| open.id);
        let id = self.nodes.len();
        self.nodes.push(Node {
            tag: "#comment".into(),
            text: text.into(),
            parent: Some(parent),
            ..Node::default()
        });
        self.nodes[parent].children.push(id);
    }

    fn merge(&mut self, index: usize, tag: &str, attrs: Attrs) {
        let Some(id) = self
            .stack
            .get(index)
            .map(|open| open.id)
            .filter(|&id| self.nodes[id].tag == tag)
        else {
            return;
        };
        for (name, value) in attrs {
            if !self.nodes[id].attrs.contains_key(&name) {
                self.nodes[id].attrs.append(name, value);
            }
        }
    }

    fn template_end(&mut self) {
        let at = self.last.get(&key("template")).copied().unwrap_or(0);
        self.pop_to(at);
    }

    fn head(&mut self, tag: &str, attrs: &mut Attrs) -> Option<Content> {
        if !matches!(
            tag,
            "base"
                | "basefont"
                | "bgsound"
                | "link"
                | "meta"
                | "title"
                | "noframes"
                | "style"
                | "script"
                | "template"
        ) && !(tag == "noscript" && self.mode == Mode::Head)
        {
            return None;
        }
        let attrs = std::mem::take(attrs);
        Some(match tag {
            "base" | "basefont" | "bgsound" | "link" | "meta" => {
                self.element(tag, attrs);
                Content::Markup
            }
            "title" => Content::Raw(self.element(tag, attrs), true),
            "template" => self.open(tag, attrs, Some(Mode::Template)),
            _ => Content::Raw(self.element(tag, attrs), false),
        })
    }

    fn start(&mut self, tag: &str, mut attrs: Attrs, closing: bool) -> Content {
        self.newline = false;
        if self.foreign() {
            if !(matches!(
                tag,
                "b" | "big"
                    | "blockquote"
                    | "body"
                    | "br"
                    | "center"
                    | "code"
                    | "dd"
                    | "div"
                    | "dl"
                    | "dt"
                    | "em"
                    | "embed"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "head"
                    | "hr"
                    | "i"
                    | "img"
                    | "li"
                    | "listing"
                    | "menu"
                    | "meta"
                    | "nobr"
                    | "ol"
                    | "p"
                    | "pre"
                    | "ruby"
                    | "s"
                    | "small"
                    | "span"
                    | "strong"
                    | "strike"
                    | "sub"
                    | "sup"
                    | "table"
                    | "tt"
                    | "u"
                    | "ul"
                    | "var"
            ) || tag == "font"
                && ["color", "face", "size"]
                    .iter()
                    .any(|name| attrs.contains_key(name)))
            {
                return self.svg(tag, attrs, closing);
            }
            while self.stack.len() > 1 && self.foreign() {
                self.pop();
            }
        }
        if tag == "html" && self.mode != Mode::Initial {
            self.merge(0, tag, attrs);
            return Content::Markup;
        }
        match self.mode {
            Mode::Initial | Mode::BeforeHead => {
                let (name, next) = if self.mode == Mode::Initial {
                    ("html", Mode::BeforeHead)
                } else {
                    ("head", Mode::Head)
                };
                let own = tag == name;
                let id = self.element(
                    name,
                    if own {
                        std::mem::take(&mut attrs)
                    } else {
                        Attrs::default()
                    },
                );
                if name == "head" {
                    self.head = Some(id);
                }
                self.push(id, false, Some(next));
                if own {
                    Content::Markup
                } else {
                    self.start(tag, attrs, closing)
                }
            }
            Mode::Head => {
                if let Some(content) = self.head(tag, &mut attrs) {
                    return content;
                }
                if tag == "head" {
                    return Content::Markup;
                }
                self.pop();
                self.mode = Mode::AfterHead;
                self.start(tag, attrs, closing)
            }
            Mode::AfterHead => {
                if let Some(head) = self.head {
                    self.push(head, false, None);
                    if let Some(content) = self.head(tag, &mut attrs) {
                        self.remove(self.stack.len() - if tag == "template" { 2 } else { 1 });
                        return content;
                    }
                    self.pop();
                }
                if tag == "head" {
                    return Content::Markup;
                }
                let own = tag == "body";
                let id = self.element(
                    "body",
                    if own {
                        std::mem::take(&mut attrs)
                    } else {
                        Attrs::default()
                    },
                );
                self.push(id, false, Some(Mode::Body));
                if own {
                    Content::Markup
                } else {
                    self.start(tag, attrs, closing)
                }
            }
            Mode::Body => self.body(tag, attrs, closing),
            Mode::Table => self.table(tag, attrs, closing),
            Mode::Caption => {
                if matches!(
                    tag,
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) {
                    let caption = self.scope("caption", TABLE);
                    if caption == 0 {
                        return Content::Markup;
                    }
                    self.pop_to(caption);
                    return self.start(tag, attrs, closing);
                }
                self.body(tag, attrs, closing)
            }
            Mode::Columns => {
                if tag == "col" {
                    self.element(tag, attrs);
                    return Content::Markup;
                }
                if tag == "template" {
                    return self.head(tag, &mut attrs).unwrap_or(Content::Markup);
                }
                if !self.is("colgroup") {
                    return Content::Markup;
                }
                self.pop();
                self.start(tag, attrs, closing)
            }
            Mode::Section => match tag {
                "tr" | "th" | "td" => {
                    self.clear(&["tbody", "tfoot", "thead", "template"]);
                    if tag == "tr" {
                        return self.open(tag, attrs, Some(Mode::Row));
                    }
                    self.open("tr", Attrs::default(), Some(Mode::Row));
                    self.start(tag, attrs, closing)
                }
                "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" => {
                    if ["tbody", "tfoot", "thead"]
                        .iter()
                        .all(|section| self.scope(section, TABLE) == 0)
                    {
                        return Content::Markup;
                    }
                    self.clear(&["tbody", "tfoot", "thead", "template"]);
                    self.pop();
                    self.start(tag, attrs, closing)
                }
                _ => self.table(tag, attrs, closing),
            },
            Mode::Row => match tag {
                "th" | "td" => {
                    self.clear(&["tr", "template"]);
                    self.open(tag, attrs, Some(Mode::Cell))
                }
                "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr" => {
                    if self.scope("tr", TABLE) == 0 {
                        return Content::Markup;
                    }
                    self.clear(&["tr", "template"]);
                    self.pop();
                    self.start(tag, attrs, closing)
                }
                _ => self.table(tag, attrs, closing),
            },
            Mode::Cell => {
                if matches!(
                    tag,
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) {
                    let cell = self.scope("td", TABLE).max(self.scope("th", TABLE));
                    if cell == 0 {
                        return Content::Markup;
                    }
                    self.pop_to(cell);
                    return self.start(tag, attrs, closing);
                }
                self.body(tag, attrs, closing)
            }
            Mode::Select => {
                let select = self.scope("select", SELECT);
                let table = select > 0
                    && matches!(
                        self.stack[select - 1].saved,
                        Some(Mode::Table | Mode::Caption | Mode::Section | Mode::Row | Mode::Cell)
                    );
                match tag {
                    "option" | "optgroup" | "hr" => {
                        if self.is("option") {
                            self.pop();
                        }
                        if tag != "option" && self.is("optgroup") {
                            self.pop();
                        }
                        if tag == "hr" {
                            self.element(tag, attrs);
                            return Content::Markup;
                        }
                        self.open(tag, attrs, None)
                    }
                    "select" => {
                        self.pop_to(select);
                        Content::Markup
                    }
                    "input" | "keygen" | "textarea" if select > 0 => {
                        self.pop_to(select);
                        self.start(tag, attrs, closing)
                    }
                    "caption" | "table" | "tbody" | "tfoot" | "thead" | "tr" | "td" | "th"
                        if table =>
                    {
                        self.pop_to(select);
                        self.start(tag, attrs, closing)
                    }
                    "script" | "style" | "template" => {
                        self.head(tag, &mut attrs).unwrap_or(Content::Markup)
                    }
                    _ => Content::Markup,
                }
            }
            Mode::Template => {
                if let Some(content) = self.head(tag, &mut attrs) {
                    return content;
                }
                self.mode = match tag {
                    "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => Mode::Table,
                    "col" => Mode::Columns,
                    "tr" => Mode::Section,
                    "td" | "th" => Mode::Row,
                    _ => Mode::Body,
                };
                self.start(tag, attrs, closing)
            }
        }
    }

    fn body(&mut self, tag: &str, mut attrs: Attrs, closing: bool) -> Content {
        if let Some(content) = self.head(tag, &mut attrs) {
            return content;
        }
        let template = tag == "form" && self.last.contains_key(&key("template"));
        if tag == "form" && self.form.is_some() && !template {
            return Content::Markup;
        }
        let heading = |tag: &str| matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        let block = heading(tag)
            || matches!(
                tag,
                "address"
                    | "article"
                    | "aside"
                    | "blockquote"
                    | "center"
                    | "details"
                    | "dialog"
                    | "dir"
                    | "div"
                    | "dl"
                    | "fieldset"
                    | "figcaption"
                    | "figure"
                    | "footer"
                    | "form"
                    | "header"
                    | "hgroup"
                    | "main"
                    | "menu"
                    | "nav"
                    | "ol"
                    | "p"
                    | "search"
                    | "section"
                    | "summary"
                    | "ul"
                    | "pre"
                    | "listing"
                    | "table"
                    | "hr"
                    | "xmp"
                    | "plaintext"
                    | "li"
                    | "dd"
                    | "dt"
            );
        if block {
            let p = self.scope("p", BUTTON);
            self.pop_to(p);
        }
        match tag {
            "body" => {
                self.merge(1, tag, attrs);
                Content::Markup
            }
            "head" | "frameset" | "frame" | "caption" | "col" | "colgroup" | "tbody" | "td"
            | "tfoot" | "th" | "thead" | "tr" => Content::Markup,
            _ if heading(tag) => {
                if self
                    .stack
                    .last()
                    .is_some_and(|open| !open.svg && heading(&self.nodes[open.id].tag))
                {
                    self.pop();
                }
                self.open(tag, attrs, None)
            }
            "pre" | "listing" => {
                self.open(tag, attrs, None);
                self.newline = true;
                Content::Markup
            }
            "li" | "dd" | "dt" => {
                for index in (1..self.stack.len()).rev() {
                    let open = &self.stack[index];
                    let name = self.nodes[open.id].tag.as_str();
                    let found = !open.svg
                        && if tag == "li" {
                            name == "li"
                        } else {
                            matches!(name, "dd" | "dt")
                        };
                    let stop =
                        open.bits & 1 << SPECIAL != 0 && !matches!(name, "address" | "div" | "p");
                    if found {
                        self.pop_to(index + 1);
                    }
                    if found || stop {
                        break;
                    }
                }
                self.open(tag, attrs, None)
            }
            "plaintext" => {
                self.open(tag, attrs, None);
                Content::Plain
            }
            "button" => {
                let open = self.scope(tag, DEFAULT);
                self.pop_to(open);
                self.reconstruct();
                self.open(tag, attrs, None)
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small"
            | "strike" | "strong" | "tt" | "u" => {
                let marker = self
                    .active
                    .iter()
                    .rposition(Option::is_none)
                    .map_or(0, |at| at + 1);
                if tag == "a"
                    && let Some((id, _)) = self.active[marker..]
                        .iter()
                        .flatten()
                        .find(|(id, _)| self.nodes[*id].tag == "a")
                        .cloned()
                {
                    self.adoption("a");
                    self.active
                        .retain(|entry| entry.as_ref().is_none_or(|(other, _)| *other != id));
                    if let Some(at) = self.stack.iter().rposition(|open| open.id == id) {
                        self.remove(at);
                    }
                }
                self.reconstruct();
                if tag == "nobr" && self.scope(tag, DEFAULT) > 0 {
                    self.adoption("nobr");
                    self.reconstruct();
                }
                let marker = self
                    .active
                    .iter()
                    .rposition(Option::is_none)
                    .map_or(0, |at| at + 1);
                let same: Vec<usize> = (marker..self.active.len())
                    .filter(|&at| matches!(&self.active[at], Some((id, other)) if self.nodes[*id].tag == tag && other.len() == attrs.len() && attrs.iter().all(|(name, value)| other.get(name) == Some(value))))
                    .collect();
                if same.len() >= 3 {
                    self.active.remove(same[0]);
                }
                let id = self.element(tag, attrs.clone());
                self.push(id, false, None);
                self.active.push(Some((id, attrs)));
                Content::Markup
            }
            "form" => {
                let id = self.element(tag, attrs);
                self.push(id, false, None);
                if !template {
                    self.form = Some(id);
                }
                Content::Markup
            }
            "table" => self.open(tag, attrs, Some(Mode::Table)),
            "area" | "br" | "embed" | "img" | "image" | "keygen" | "wbr" | "input" | "param"
            | "source" | "track" | "hr" => {
                if !matches!(tag, "param" | "source" | "track" | "hr") {
                    self.reconstruct();
                }
                self.element(if tag == "image" { "img" } else { tag }, attrs);
                Content::Markup
            }
            "textarea" => Content::Raw(self.element(tag, attrs), true),
            "xmp" | "iframe" | "noembed" | "noscript" => {
                if tag == "xmp" {
                    self.reconstruct();
                }
                Content::Raw(self.element(tag, attrs), false)
            }
            "select" => {
                self.reconstruct();
                self.open(tag, attrs, Some(Mode::Select))
            }
            "optgroup" | "option" => {
                if self.is("option") {
                    self.pop();
                }
                self.reconstruct();
                self.open(tag, attrs, None)
            }
            "rb" | "rtc" | "rp" | "rt" => {
                let except = if matches!(tag, "rp" | "rt") {
                    "rtc"
                } else {
                    ""
                };
                while self.scope("ruby", DEFAULT) > 0
                    && self.stack.len() > 1
                    && self.stack.last().is_some_and(|open| {
                        let name = self.nodes[open.id].tag.as_str();
                        !open.svg && name != except && IMPLIED.contains(&name)
                    })
                {
                    self.pop();
                }
                self.open(tag, attrs, None)
            }
            "svg" => {
                self.reconstruct();
                self.svg(tag, attrs, closing)
            }
            _ => {
                if !block {
                    self.reconstruct();
                }
                let id = self.element(tag, attrs);
                self.push(id, false, None);
                Content::Markup
            }
        }
    }

    fn table(&mut self, tag: &str, mut attrs: Attrs, closing: bool) -> Content {
        let context = ["table", "template"];
        match tag {
            "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => {
                self.clear(&context);
                let mode = match tag {
                    "caption" => Mode::Caption,
                    "colgroup" => Mode::Columns,
                    _ => Mode::Section,
                };
                self.open(tag, attrs, Some(mode))
            }
            "col" | "td" | "th" | "tr" => {
                self.clear(&context);
                if tag == "col" {
                    self.open("colgroup", Attrs::default(), Some(Mode::Columns));
                } else {
                    self.open("tbody", Attrs::default(), Some(Mode::Section));
                }
                self.start(tag, attrs, closing)
            }
            "table" => {
                let table = self.scope("table", TABLE);
                if table == 0 {
                    return Content::Markup;
                }
                self.pop_to(table);
                self.start(tag, attrs, closing)
            }
            "style" | "script" | "template" => {
                self.head(tag, &mut attrs).unwrap_or(Content::Markup)
            }
            "input"
                if attrs
                    .get("type")
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("hidden")) =>
            {
                self.element(tag, attrs);
                Content::Markup
            }
            "form" => {
                if self.form.is_none() && !self.last.contains_key(&key("template")) {
                    self.form = Some(self.element(tag, attrs));
                }
                Content::Markup
            }
            _ => {
                self.foster = true;
                let content = self.body(tag, attrs, closing);
                self.foster = false;
                content
            }
        }
    }

    fn end(&mut self, tag: &str) {
        self.newline = false;
        if self.stack.last().is_some_and(|open| open.svg) {
            if !matches!(tag, "br" | "p") {
                if let Some(&at) = self.last.get(&key(tag))
                    && at > self.bound[HTML]
                {
                    self.pop_to(at);
                    return;
                }
            } else {
                while self.stack.len() > 1 && self.foreign() {
                    self.pop();
                }
            }
        }
        let sections = ["tbody", "tfoot", "thead", "template"];
        match self.mode {
            Mode::Initial | Mode::BeforeHead => {}
            Mode::Head | Mode::AfterHead => match tag {
                "template" => self.template_end(),
                "head" if self.mode == Mode::Head => {
                    self.pop();
                    self.mode = Mode::AfterHead;
                }
                "body" | "html" | "br" => {
                    if self.mode == Mode::Head {
                        self.pop();
                        self.mode = Mode::AfterHead;
                    } else {
                        let id = self.element("body", Attrs::default());
                        self.push(id, false, Some(Mode::Body));
                    }
                    self.end(tag);
                }
                _ => {}
            },
            Mode::Body => self.body_end(tag),
            Mode::Table => self.table_end(tag),
            Mode::Caption => match tag {
                "caption" | "table" => {
                    let caption = self.scope("caption", TABLE);
                    if caption > 0 {
                        self.pop_to(caption);
                        if tag == "table" {
                            self.end(tag);
                        }
                    }
                }
                "body" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th"
                | "thead" | "tr" => {}
                _ => self.body_end(tag),
            },
            Mode::Columns => match tag {
                "colgroup" if self.is("colgroup") => self.pop(),
                "colgroup" | "col" => {}
                "template" => self.template_end(),
                _ if self.is("colgroup") => {
                    self.pop();
                    self.end(tag);
                }
                _ => {}
            },
            Mode::Section => match tag {
                "tbody" | "tfoot" | "thead" | "table" => {
                    let open = if tag == "table" {
                        sections[..3]
                            .iter()
                            .any(|section| self.scope(section, TABLE) > 0)
                    } else {
                        self.scope(tag, TABLE) > 0
                    };
                    if open {
                        self.clear(&sections);
                        self.pop();
                        if tag == "table" {
                            self.end(tag);
                        }
                    }
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr" => {}
                _ => self.table_end(tag),
            },
            Mode::Row => match tag {
                "tr" | "table" | "tbody" | "tfoot" | "thead" => {
                    if self.scope("tr", TABLE) > 0
                        && (matches!(tag, "tr" | "table") || self.scope(tag, TABLE) > 0)
                    {
                        self.clear(&["tr", "template"]);
                        self.pop();
                        if tag != "tr" {
                            self.end(tag);
                        }
                    }
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" => {}
                _ => self.table_end(tag),
            },
            Mode::Cell => match tag {
                "td" | "th" => {
                    let cell = self.scope(tag, TABLE);
                    self.pop_to(cell);
                }
                "body" | "caption" | "col" | "colgroup" | "html" => {}
                "table" | "tbody" | "tfoot" | "thead" | "tr" => {
                    if self.scope(tag, TABLE) > 0 {
                        let cell = self.scope("td", TABLE).max(self.scope("th", TABLE));
                        self.pop_to(cell);
                        self.end(tag);
                    }
                }
                _ => self.body_end(tag),
            },
            Mode::Select => {
                let select = self.scope("select", SELECT);
                match tag {
                    "optgroup" => {
                        if self.is("option")
                            && self.stack.len() > 1
                            && self.nodes[self.stack[self.stack.len() - 2].id].tag == "optgroup"
                        {
                            self.pop();
                        }
                        if self.is("optgroup") {
                            self.pop();
                        }
                    }
                    "option" if self.is("option") => self.pop(),
                    "select" => self.pop_to(select),
                    "template" => self.template_end(),
                    "caption" | "table" | "tbody" | "tfoot" | "thead" | "tr" | "td" | "th"
                        if select > 0
                            && matches!(
                                self.stack[select - 1].saved,
                                Some(
                                    Mode::Table
                                        | Mode::Caption
                                        | Mode::Section
                                        | Mode::Row
                                        | Mode::Cell
                                )
                            )
                            && self.scope(tag, TABLE) > 0 =>
                    {
                        self.pop_to(select);
                        self.end(tag);
                    }
                    _ => {}
                }
            }
            Mode::Template => {
                if tag == "template" {
                    self.template_end();
                }
            }
        }
    }

    fn table_end(&mut self, tag: &str) {
        match tag {
            "table" => {
                let table = self.scope("table", TABLE);
                self.pop_to(table);
            }
            "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th"
            | "thead" | "tr" => {}
            "template" => self.template_end(),
            _ => {
                self.foster = true;
                self.body_end(tag);
                self.foster = false;
            }
        }
    }

    fn body_end(&mut self, tag: &str) {
        let at = match tag {
            "template" => return self.template_end(),
            "body" | "html" => return,
            "br" => {
                self.reconstruct();
                self.element(tag, Attrs::default());
                return;
            }
            "form" if !self.last.contains_key(&key("template")) => {
                let Some(form) = self.form.take() else { return };
                let at = self.scope(tag, DEFAULT);
                if at == 0 || self.stack[at - 1].id != form {
                    return;
                }
                while self.stack.last().is_some_and(|open| {
                    !open.svg && IMPLIED.contains(&self.nodes[open.id].tag.as_str())
                }) {
                    self.pop();
                }
                return self.remove(at - 1);
            }
            "p" => {
                if self.scope(tag, BUTTON) == 0 {
                    self.open(tag, Attrs::default(), None);
                }
                self.scope(tag, BUTTON)
            }
            "li" => self.scope(tag, LIST),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => ["h1", "h2", "h3", "h4", "h5", "h6"]
                .iter()
                .map(|heading| self.scope(heading, DEFAULT))
                .max()
                .unwrap_or(0),
            "address" | "applet" | "article" | "aside" | "blockquote" | "button" | "center"
            | "dd" | "details" | "dialog" | "dir" | "div" | "dl" | "dt" | "fieldset"
            | "figcaption" | "figure" | "footer" | "form" | "header" | "hgroup" | "listing"
            | "main" | "marquee" | "menu" | "nav" | "object" | "ol" | "pre" | "search"
            | "section" | "summary" | "ul" => self.scope(tag, DEFAULT),
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small"
            | "strike" | "strong" | "tt" | "u" => return self.adoption(tag),
            _ => self
                .last
                .get(&key(tag))
                .copied()
                .filter(|&at| at >= self.bound[SPECIAL])
                .unwrap_or(0),
        };
        self.pop_to(at);
    }

    fn text(&mut self, text: &str) {
        let text = if std::mem::take(&mut self.newline) {
            text.strip_prefix('\n').unwrap_or(text)
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        if self.foreign() {
            return self.insert(text);
        }
        let rest = text.trim_start_matches(['\t', '\n', '\x0c', ' ']);
        match self.mode {
            Mode::Initial | Mode::BeforeHead | Mode::Head | Mode::AfterHead | Mode::Columns => {
                if matches!(self.mode, Mode::Head | Mode::AfterHead | Mode::Columns)
                    && rest.len() < text.len()
                {
                    self.insert(&text[..text.len() - rest.len()]);
                }
                if rest.is_empty() {
                    return;
                }
                match self.mode {
                    Mode::Initial => {
                        let id = self.element("html", Attrs::default());
                        self.push(id, false, Some(Mode::BeforeHead));
                    }
                    Mode::BeforeHead => {
                        let id = self.element("head", Attrs::default());
                        self.head = Some(id);
                        self.push(id, false, Some(Mode::Head));
                    }
                    Mode::Head => {
                        self.pop();
                        self.mode = Mode::AfterHead;
                    }
                    Mode::AfterHead => {
                        let id = self.element("body", Attrs::default());
                        self.push(id, false, Some(Mode::Body));
                    }
                    _ if self.is("colgroup") => self.pop(),
                    _ => return,
                }
                self.text(rest);
            }
            Mode::Table | Mode::Section | Mode::Row => {
                self.foster = !rest.is_empty();
                if self.foster {
                    self.reconstruct();
                }
                self.insert(text);
                self.foster = false;
            }
            Mode::Body | Mode::Caption | Mode::Cell => {
                self.reconstruct();
                self.insert(text);
            }
            _ => self.insert(text),
        }
    }
}

impl Document {
    pub fn parse(source: &str) -> Self {
        Self::build(source, None, false)
    }

    pub fn fragment(source: &str, context: &str) -> Self {
        Self::build(source, Some(context), false)
    }

    // `form` is true when the context element is in a form, so the fragment can't open another one.
    pub(crate) fn build(source: &str, context: Option<&str>, form: bool) -> Self {
        let normalized;
        let source = if source.contains('\r') {
            normalized = source.replace("\r\n", "\n").replace('\r', "\n");
            normalized.as_str()
        } else {
            source
        };
        let mut nodes = Vec::with_capacity(context.map_or(1, |_| source.len() / 32 + 1));
        nodes.push(Node {
            tag: if context.is_some() {
                "#fragment"
            } else {
                "#document"
            }
            .into(),
            ..Node::default()
        });
        let mut builder = Builder {
            nodes,
            stack: Vec::new(),
            active: Vec::new(),
            opened: Vec::new(),
            last: rustc_hash::FxHashMap::default(),
            bound: [0; 7],
            mode: Mode::Initial,
            head: None,
            // Node 0 is never an open form, so `</form>` ignores this pointer.
            form: form.then_some(0),
            foster: false,
            newline: false,
        };
        if let Some(context) = context {
            let raw = RAW.contains(&context);
            if raw || matches!(context, "title" | "textarea") {
                if !source.is_empty() {
                    builder.nodes.push(Node {
                        tag: "#text".into(),
                        text: if raw {
                            source.into()
                        } else {
                            decode(source, false)
                        },
                        parent: Some(0),
                        ..Node::default()
                    });
                    builder.nodes[0].children.push(1);
                }
                return Self {
                    nodes: builder.nodes.into(),
                    target: None,
                };
            }
            builder.push(0, context == "svg", None);
            builder.bound = [1, 1, 1, 1, 1, 1, usize::from(context != "svg")];
            builder.mode = match context {
                "html" => Mode::BeforeHead,
                "select" => Mode::Select,
                "td" | "th" => Mode::Cell,
                "tr" => Mode::Row,
                "tbody" | "tfoot" | "thead" => Mode::Section,
                "caption" => Mode::Caption,
                "colgroup" => Mode::Columns,
                "table" => Mode::Table,
                "template" => Mode::Template,
                _ => Mode::Body,
            };
        }
        let bytes = source.as_bytes();
        let space = |c: u8| matches!(c, b'\t' | b'\n' | b'\x0c' | b' ');
        let mut pos = 0;
        while pos < bytes.len() {
            let rest = &source[pos..];
            let next = bytes.get(pos + 1).copied().unwrap_or(0);
            if bytes[pos] != b'<'
                || !(next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?'))
            {
                let skip = usize::from(bytes[pos] == b'<');
                let end = source[pos + skip..]
                    .find('<')
                    .map_or(bytes.len(), |n| pos + skip + n);
                builder.text(&decode(&source[pos..end], false));
                pos = end;
            } else if next.is_ascii_alphabetic() {
                let Some((name, attrs, closing, length)) = tag(&rest[1..]) else {
                    break;
                };
                pos += 1 + length;
                match builder.start(&name, attrs, closing) {
                    Content::Markup => {}
                    Content::Plain => {
                        builder.text(&source[pos..]);
                        break;
                    }
                    Content::Raw(id, references) => {
                        let script = name == "script";
                        let mut state = 0;
                        let mut at = pos;
                        let close = loop {
                            let found = if state == 0 {
                                source[at..].find('<')
                            } else {
                                source[at..].find(['<', '-'])
                            };
                            let Some(n) = found else { break bytes.len() };
                            at += n;
                            let after = &bytes[at..];
                            let word = |from: usize| {
                                after
                                    .get(from..from + name.len())
                                    .is_some_and(|word| word.eq_ignore_ascii_case(name.as_bytes()))
                                    && after
                                        .get(from + name.len())
                                        .is_none_or(|&c| space(c) || c == b'/' || c == b'>')
                            };
                            match after {
                                [b'-', b'-', b'>', ..] => state = 0,
                                [b'<', b'!', b'-', b'-', ..] if script && state == 0 => {
                                    state = 1;
                                    at += 2;
                                    continue;
                                }
                                [b'<', b'/', ..] if word(2) => {
                                    if state < 2 {
                                        break at;
                                    }
                                    state = 1;
                                }
                                [b'<', ..] if state == 1 && word(1) => state = 2,
                                _ => {}
                            }
                            at += 1;
                        };
                        let text = &source[pos..close];
                        let text = if name == "textarea" {
                            text.strip_prefix('\n').unwrap_or(text)
                        } else {
                            text
                        };
                        if !text.is_empty() {
                            let child = builder.nodes.len();
                            builder.nodes.push(Node {
                                tag: "#text".into(),
                                text: if references {
                                    decode(text, false)
                                } else {
                                    text.into()
                                },
                                parent: Some(id),
                                ..Node::default()
                            });
                            builder.nodes[id].children.push(child);
                        }
                        pos = close;
                        if close < bytes.len() {
                            let Some((.., length)) = tag(&source[close + 2..]) else {
                                break;
                            };
                            pos += 2 + length;
                        }
                    }
                }
            } else if next == b'/' {
                match bytes.get(pos + 2) {
                    Some(c) if c.is_ascii_alphabetic() => {
                        let Some((name, .., length)) = tag(&rest[2..]) else {
                            break;
                        };
                        pos += 2 + length;
                        builder.end(&name);
                    }
                    Some(b'>') => pos += 3,
                    None => {
                        builder.text("</");
                        break;
                    }
                    _ => {
                        let close = rest.find('>').unwrap_or(rest.len());
                        builder.comment(&rest[2..close]);
                        pos += (close + 1).min(rest.len());
                    }
                }
            } else if let Some(body) = rest.strip_prefix("<!--") {
                let (data, length) = if body.starts_with('>') {
                    ("", 5)
                } else if body.starts_with("->") {
                    ("", 6)
                } else {
                    let mut from = 0;
                    loop {
                        let Some(at) = body[from..].find("--").map(|n| from + n) else {
                            break (body, rest.len());
                        };
                        match body.as_bytes()[at + 2..] {
                            [b'>', ..] => break (&body[..at], at + 7),
                            [b'!', b'>', ..] => break (&body[..at], at + 8),
                            _ => from = at + 1,
                        }
                    }
                };
                builder.comment(data);
                pos += length;
            } else if let Some(body) = rest
                .strip_prefix("<![CDATA[")
                .filter(|_| builder.stack.last().is_some_and(|open| open.svg))
            {
                let end = body.find("]]>").unwrap_or(body.len());
                builder.insert(&body[..end]);
                pos += (9 + end + 3).min(rest.len());
            } else {
                let close = rest.find('>').unwrap_or(rest.len());
                if !bytes
                    .get(pos + 2..pos + 9)
                    .is_some_and(|word| word.eq_ignore_ascii_case(b"doctype"))
                {
                    builder.comment(&rest[if next == b'?' { 1 } else { 2 }..close]);
                } else if context.is_none()
                    && matches!(builder.mode, Mode::Initial)
                    && builder.stack.is_empty()
                {
                    let body = rest[9.min(close)..close].trim_ascii();
                    let (name, tail) = body
                        .split_once(|c: char| c.is_ascii_whitespace())
                        .unwrap_or((body, ""));
                    let quoted = |text: &str| -> Option<(String, String)> {
                        let text = text.trim_ascii_start();
                        let quote = text.chars().next().filter(|c| matches!(c, '"' | '\''))?;
                        let end = text[1..].find(quote).map_or(text.len(), |end| end + 1);
                        Some((
                            text[1..end].into(),
                            text.get(end + 1..).unwrap_or("").into(),
                        ))
                    };
                    let tail = tail.trim_ascii_start();
                    let ids = match tail.get(..6).map(str::to_ascii_lowercase).as_deref() {
                        Some("public") => quoted(&tail[6..]).map(|(public, rest)| {
                            (
                                public,
                                quoted(&rest).map(|(system, _)| system).unwrap_or_default(),
                            )
                        }),
                        Some("system") => {
                            quoted(&tail[6..]).map(|(system, _)| (String::new(), system))
                        }
                        _ => None,
                    };
                    let mut text = name.to_ascii_lowercase();
                    if let Some((public, system)) = ids {
                        text = format!("{text}\0{public}\0{system}");
                    }
                    let id = builder.nodes.len();
                    builder.nodes.push(Node {
                        tag: "#doctype".into(),
                        text,
                        parent: Some(0),
                        ..Node::default()
                    });
                    builder.nodes[0].children.push(id);
                }
                pos += (close + 1).min(rest.len());
            }
        }
        if context.is_none()
            && !builder.stack.first().is_some_and(|html| {
                builder.nodes[html.id]
                    .children
                    .iter()
                    .any(|&child| builder.nodes[child].tag == "body")
            })
        {
            builder.pop_to(2);
            if builder.head.is_some() {
                builder.mode = Mode::AfterHead;
            }
            builder.start("body", Attrs::default(), false);
        }
        Self {
            nodes: builder.nodes.into(),
            target: None,
        }
    }
}

fn decode(value: &str, attribute: bool) -> String {
    const C1: [u16; 32] = [
        0x20ac, 0, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
        0x152, 0, 0x17d, 0, 0, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc,
        0x2122, 0x161, 0x203a, 0x153, 0, 0x17e, 0x178,
    ];
    let entity = |name: &str| -> Option<&'static str> {
        let data = entities::ENTITIES;
        let (mut low, mut high) = (0, data.len());
        while low < high {
            let middle = (low + high) / 2;
            let start = data.as_bytes()[..middle]
                .iter()
                .rposition(|&byte| byte == 0)
                .map_or(0, |index| index + 1);
            let end = data[start..].find('\0').map_or(data.len(), |n| start + n);
            let (key, value) = data[start..end].split_once(';')?;
            match key.cmp(name) {
                std::cmp::Ordering::Equal => return Some(value),
                std::cmp::Ordering::Less => low = end + 1,
                std::cmp::Ordering::Greater => high = start,
            }
        }
        None
    };
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let bytes = rest.as_bytes();
        if bytes.first() == Some(&b'#') {
            let hex = matches!(bytes.get(1), Some(b'x' | b'X'));
            let digits = &rest[1 + usize::from(hex)..];
            let count = digits
                .bytes()
                .take_while(|c| {
                    if hex {
                        c.is_ascii_hexdigit()
                    } else {
                        c.is_ascii_digit()
                    }
                })
                .count();
            if count == 0 {
                result.push('&');
                continue;
            }
            let code = digits[..count].bytes().fold(0u32, |code, digit| {
                (code * if hex { 16 } else { 10 } + char::from(digit).to_digit(16).unwrap_or(0))
                    .min(0x110000)
            });
            rest = &digits[count..];
            rest = rest.strip_prefix(';').unwrap_or(rest);
            let code = match code {
                0x80..=0x9f if C1[code as usize - 0x80] != 0 => u32::from(C1[code as usize - 0x80]),
                _ => code,
            };
            result.push(
                char::from_u32(code)
                    .filter(|_| code != 0)
                    .unwrap_or('\u{fffd}'),
            );
            continue;
        }
        let name = bytes
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        let found = if bytes.get(name) == Some(&b';') {
            entity(&rest[..name]).map(|value| (value, name + 1))
        } else {
            None
        }
        .or_else(|| {
            (2..=name.min(6))
                .rev()
                .find(|&length| {
                    entities::LEGACY
                        .split(' ')
                        .any(|legacy| legacy == &rest[..length])
                })
                .and_then(|length| Some((entity(&rest[..length])?, length)))
                .filter(|&(_, length)| {
                    !attribute
                        || !bytes
                            .get(length)
                            .is_some_and(|&c| c == b'=' || c.is_ascii_alphanumeric())
                })
        });
        match found {
            Some((value, length)) => {
                result.push_str(value);
                rest = &rest[length..];
            }
            None => result.push('&'),
        }
    }
    result.push_str(rest);
    result
}
