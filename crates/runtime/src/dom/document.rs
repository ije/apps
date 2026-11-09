use super::Selector;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Attrs(Vec<(String, String)>);

impl Attrs {
    pub fn get(&self, name: &str) -> Option<&String> {
        self.0
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut String> {
        self.0
            .iter_mut()
            .find_map(|(key, value)| (key == name).then_some(value))
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.0.iter().any(|(key, _)| key == name)
    }

    pub fn insert(&mut self, name: String, value: String) -> Option<String> {
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some((_, old)) => Some(std::mem::replace(old, value)),
            None => {
                self.0.push((name, value));
                None
            }
        }
    }

    pub fn set(&mut self, name: &str, value: &str) {
        match self.get_mut(name) {
            Some(old) => value.clone_into(old),
            None => self.0.push((name.into(), value.into())),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<String> {
        let index = self.0.iter().position(|(key, _)| key == name)?;
        Some(self.0.remove(index).1)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter().map(|(key, value)| (key, value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(key, _)| key)
    }
}

impl<'a> IntoIterator for &'a Attrs {
    type Item = (&'a String, &'a String);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, String)>,
        fn(&'a (String, String)) -> (&'a String, &'a String),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(key, value)| (key, value))
    }
}

impl std::ops::Index<&str> for Attrs {
    type Output = String;

    fn index(&self, name: &str) -> &String {
        self.get(name).expect("missing attribute")
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Node {
    pub tag: String,
    pub text: String,
    pub attrs: Attrs,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub(crate) stylesheet: Option<Rc<crate::style::Stylesheet>>,
    // WARNING: the `style` attribute stays in `attrs`; this is its parse, kept only while the attribute is unchanged, so every write of `style` must clear it.
    pub(crate) inline: Option<crate::style::Declarations>,
    #[serde(skip)]
    pub(crate) state: u16,
    #[serde(skip)]
    pub(crate) value: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Document {
    pub nodes: Nodes,
}

static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn stamp() -> u64 {
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// WARNING: revisions are unique across documents, so a cache never mistakes one node for another.
#[derive(Clone, Debug, Default)]
pub struct Nodes {
    list: Vec<Node>,
    revisions: Vec<u64>,
    latest: u64,
    tree: u64,
}

impl Nodes {
    pub(crate) fn truncate(&mut self, len: usize) {
        self.list.truncate(len);
        self.list.shrink_to_fit();
        self.revisions.truncate(len);
        self.revisions.shrink_to_fit();
        self.latest = stamp();
        self.tree = self.latest;
    }

    pub fn push(&mut self, node: Node) {
        self.list.push(node);
        self.latest = stamp();
        self.tree = self.latest;
        self.revisions.push(self.latest);
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Node> {
        let node = self.list.get_mut(id)?;
        self.latest = stamp();
        self.tree = self.latest;
        self.revisions[id] = self.latest;
        Some(node)
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Node> {
        self.revisions.fill_with(stamp);
        self.latest = stamp();
        self.tree = self.latest;
        self.list.iter_mut()
    }

    // WARNING: walks of the whole tree are kept until the tree revision changes, so the node's `parent` and
    // `children` must stay. All other mutable accesses count as tree changes.
    pub(crate) fn edit(&mut self, id: usize) -> Option<&mut Node> {
        let node = self.list.get_mut(id)?;
        self.latest = stamp();
        self.revisions[id] = self.latest;
        Some(node)
    }

    pub(crate) fn tree(&self) -> u64 {
        self.tree
    }

    // WARNING: the revision stays, so the frame must put the old children back before it ends.
    pub fn compose_children(&mut self, id: usize, children: Vec<usize>) -> Vec<usize> {
        std::mem::replace(&mut self.list[id].children, children)
    }

    // WARNING: only for passes that change no other node, so nothing misses a real change.
    #[cfg(test)]
    pub(crate) fn restore(&mut self, revisions: &[(usize, u64)], latest: u64, tree: u64) {
        for &(id, revision) in revisions {
            self.revisions[id] = revision;
        }
        self.latest = latest;
        self.tree = tree;
    }

    pub fn latest(&self) -> u64 {
        self.latest
    }

    pub fn revision(&self, id: usize) -> u64 {
        self.revisions[id]
    }
}

impl std::ops::Deref for Nodes {
    type Target = [Node];

    fn deref(&self) -> &[Node] {
        &self.list
    }
}

impl std::ops::Index<usize> for Nodes {
    type Output = Node;

    fn index(&self, id: usize) -> &Node {
        &self.list[id]
    }
}

impl std::ops::IndexMut<usize> for Nodes {
    fn index_mut(&mut self, id: usize) -> &mut Node {
        self.latest = stamp();
        self.tree = self.latest;
        self.revisions[id] = self.latest;
        &mut self.list[id]
    }
}

impl From<Vec<Node>> for Nodes {
    fn from(list: Vec<Node>) -> Self {
        let revisions = list.iter().map(|_| stamp()).collect();
        let latest = stamp();
        Self {
            list,
            revisions,
            latest,
            tree: latest,
        }
    }
}

impl IntoIterator for Nodes {
    type Item = Node;
    type IntoIter = std::vec::IntoIter<Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.into_iter()
    }
}

impl<'a> IntoIterator for &'a Nodes {
    type Item = &'a Node;
    type IntoIter = std::slice::Iter<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.iter()
    }
}

impl<'a> IntoIterator for &'a mut Nodes {
    type Item = &'a mut Node;
    type IntoIter = std::slice::IterMut<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl Serialize for Nodes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.list.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Nodes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<Node>::deserialize(deserializer).map(Self::from)
    }
}

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

// WARNING: scope checks compare 1-based stack positions: the topmost open element of each tag
// (`last`) against the topmost boundary element of each scope (`bound`), so a misnested or
// unmatched tag costs O(1) and parsing stays linear.
struct Builder {
    nodes: Vec<Node>,
    stack: Vec<Open>,
    active: Vec<Option<(usize, Attrs)>>,
    opened: Vec<bool>,
    last: rustc_hash::FxHashMap<u64, usize>,
    bound: [usize; 7],
    mode: Mode,
    head: Option<usize>,
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
        if !attrs.contains_key(&key) {
            attrs.0.push((key, value));
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
        let bits = u8::from(default)
            | u8::from(default || html && matches!(tag, "ol" | "ul")) << LIST
            | u8::from(default || html && tag == "button") << BUTTON
            | u8::from(html && matches!(tag, "html" | "table" | "template")) << TABLE
            | u8::from(!(html && matches!(tag, "optgroup" | "option"))) << SELECT
            | u8::from(special) << SPECIAL
            | u8::from(html) << HTML;
        let key = key(tag);
        let position = self.stack.len() + 1;
        let prev = self.last.insert(key, position).unwrap_or(0);
        let below = self.bound;
        for (kind, bound) in self.bound.iter_mut().enumerate() {
            if bits & 1 << kind != 0 {
                *bound = position;
            }
        }
        let saved = mode.map(|mode| std::mem::replace(&mut self.mode, mode));
        if html
            && matches!(
                tag,
                "applet" | "caption" | "marquee" | "object" | "td" | "th" | "template"
            )
        {
            self.active.push(None);
        }
        if self.opened.len() <= id {
            self.opened.resize(id + 1, false);
        }
        self.opened[id] = true;
        self.stack.push(Open {
            id,
            key,
            svg,
            bits,
            prev,
            below,
            saved,
        });
    }

    fn pop(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(open) = self.stack.pop() else { return };
        self.opened[open.id] = false;
        if !open.svg
            && matches!(
                self.nodes[open.id].tag.as_str(),
                "applet" | "caption" | "marquee" | "object" | "td" | "th" | "template"
            )
        {
            while let Some(Some(_)) = self.active.pop() {}
        }
        if open.prev == 0 {
            self.last.remove(&open.key);
        } else {
            self.last.insert(open.key, open.prev);
        }
        self.bound = open.below;
        if let Some(mode) = open.saved {
            self.mode = mode;
        }
    }

    fn pop_to(&mut self, position: usize) {
        while position > 1 && self.stack.len() >= position {
            self.pop();
        }
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

    fn place(&self) -> (usize, Option<usize>) {
        let target = self.stack.last().map_or(0, |open| open.id);
        if self.foster
            && self.stack.last().is_some_and(|open| !open.svg)
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
                            .position(|&child| child == id),
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

    fn element(&mut self, tag: &str, attrs: Attrs) -> usize {
        let (parent, before) = self.place();
        let id = self.nodes.len();
        self.nodes.push(Node {
            tag: tag.into(),
            attrs,
            parent: Some(parent),
            ..Node::default()
        });
        let children = &mut self.nodes[parent].children;
        match before {
            Some(index) => children.insert(index, id),
            None => children.push(id),
        }
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
        let attrs = Attrs(
            attrs
                .0
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
                .collect(),
        );
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

    // WARNING: it edits the stack in the middle, so the open elements from the formatting element up
    // are taken off and pushed again, which recomputes the scope positions.
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
            let common = self.stack[position - 1].id;
            let mut tail = self.stack.split_off(position);
            for open in tail.iter().rev() {
                match open.prev {
                    0 => self.last.remove(&open.key),
                    prev => self.last.insert(open.key, prev),
                };
                self.bound = open.below;
            }
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
            self.adopt(last, common);
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
            for open in tail {
                let at = self.stack.len() + 1;
                let prev = self.last.insert(open.key, at).unwrap_or(0);
                let below = self.bound;
                for (kind, bound) in self.bound.iter_mut().enumerate() {
                    if open.bits & 1 << kind != 0 {
                        *bound = at;
                    }
                }
                self.opened[open.id] = true;
                self.stack.push(Open {
                    prev,
                    below,
                    ..open
                });
            }
        }
    }

    fn insert(&mut self, text: &str) {
        let (parent, before) = self.place();
        let children = &self.nodes[parent].children;
        let previous = match before {
            Some(index) => index.checked_sub(1).map(|index| children[index]),
            None => children.last().copied(),
        };
        match previous.filter(|&previous| self.nodes[previous].tag == "#text") {
            Some(previous) => self.nodes[previous].text.push_str(text),
            None => {
                let id = self.element("#text", Attrs::default());
                self.nodes[id].text = text.into();
            }
        }
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
        for (name, value) in attrs.0 {
            if !self.nodes[id].attrs.contains_key(&name) {
                self.nodes[id].attrs.0.push((name, value));
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
                if let Some(head) = self.head
                    && matches!(
                        tag,
                        "base"
                            | "basefont"
                            | "bgsound"
                            | "link"
                            | "meta"
                            | "noframes"
                            | "script"
                            | "style"
                            | "title"
                    )
                {
                    self.push(head, false, None);
                    let content = self.head(tag, &mut attrs);
                    self.pop();
                    return content.unwrap_or(Content::Markup);
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
        let heading = |tag: &str| matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        if heading(tag)
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
            )
        {
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
                        self.pop_to(at + 1);
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
                    .filter(|&at| matches!(&self.active[at], Some((id, other)) if self.nodes[*id].tag == tag && *other == attrs))
                    .collect();
                if same.len() >= 3 {
                    self.active.remove(same[0]);
                }
                let id = self.element(tag, attrs.clone());
                self.push(id, false, None);
                self.active.push(Some((id, attrs)));
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
                        !open.svg
                            && name != except
                            && matches!(
                                name,
                                "dd" | "dt"
                                    | "li"
                                    | "optgroup"
                                    | "option"
                                    | "p"
                                    | "rb"
                                    | "rp"
                                    | "rt"
                                    | "rtc"
                            )
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
                self.reconstruct();
                let id = self.element(tag, attrs);
                if self.nodes[id].sf_symbol().is_none() {
                    self.push(id, false, None);
                }
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
            "form" => Content::Markup,
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
                self.element(tag, Attrs::default());
                return;
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
        Self::build(source, None)
    }

    pub fn fragment(source: &str, context: &str) -> Self {
        Self::build(source, Some(context))
    }

    fn build(source: &str, context: Option<&str>) -> Self {
        let normalized;
        let source = if source.contains('\r') {
            normalized = source.replace("\r\n", "\n").replace('\r', "\n");
            normalized.as_str()
        } else {
            source
        };
        let mut builder = Builder {
            nodes: vec![Node {
                tag: if context.is_some() {
                    "#fragment"
                } else {
                    "#document"
                }
                .into(),
                ..Node::default()
            }],
            stack: Vec::new(),
            active: Vec::new(),
            opened: Vec::new(),
            last: rustc_hash::FxHashMap::default(),
            bound: [0; 7],
            mode: Mode::Initial,
            head: None,
            foster: false,
            newline: false,
        };
        if let Some(context) = context {
            let raw = matches!(
                context,
                "style"
                    | "script"
                    | "xmp"
                    | "iframe"
                    | "noembed"
                    | "noframes"
                    | "noscript"
                    | "plaintext"
            );
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
                        let mut from = pos;
                        let close = loop {
                            let Some(at) = source[from..].find("</").map(|n| from + n) else {
                                break bytes.len();
                            };
                            let after = &bytes[at + 2..];
                            if after.len() >= name.len()
                                && after[..name.len()].eq_ignore_ascii_case(name.as_bytes())
                                && after
                                    .get(name.len())
                                    .is_none_or(|&c| space(c) || c == b'/' || c == b'>')
                            {
                                break at;
                            }
                            from = at + 2;
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
                    body.find("-->")
                        .map_or((body, rest.len()), |n| (&body[..n], n + 7))
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
        Self {
            nodes: builder.nodes.into(),
        }
    }

    pub fn title(&self) -> Option<String> {
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id];
            if node.tag == "title" {
                return Some(
                    self.text(id)
                        .split_ascii_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            pending.extend(node.children.iter().rev());
        }
        None
    }

    pub fn text(&self, root: usize) -> String {
        let mut text = String::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id];
            if id != root && matches!(node.tag.as_str(), "#comment" | "#doctype") {
                continue;
            }
            match &node.stylesheet {
                Some(sheet) if node.tag == "#text" => text.push_str(&sheet.source()),
                _ => text.push_str(&node.text),
            }
            pending.extend(node.children.iter().rev());
        }
        text
    }

    pub fn serialize(&self, id: usize) -> String {
        self.write(id, false, None)
    }

    // WARNING: serializes the subtree for an SVG source, without comments because XML rejects `--` in them. With
    // `mark`, each element without an id gets the id `apps-<node>`, so SVG geometry can be traced back to
    // nodes. `attrs` gives the attributes that replace an element's own, like its `style`; an empty value
    // drops the attribute. Elements never keep their own `style`.
    pub fn serialize_svg(
        &self,
        id: usize,
        mark: bool,
        attrs: &dyn Fn(usize) -> Vec<(&'static str, String)>,
    ) -> String {
        self.write(id, mark, Some(attrs))
    }

    fn write(
        &self,
        id: usize,
        mark: bool,
        svg: Option<&dyn Fn(usize) -> Vec<(&'static str, String)>>,
    ) -> String {
        let mut result = String::new();
        let mut pending = vec![(id, false)];
        while let Some((id, close)) = pending.pop() {
            let node = &self.nodes[id];
            if close {
                result.push_str("</");
                result.push_str(&node.tag);
                result.push('>');
                continue;
            }
            match node.tag.as_str() {
                "#text" => {
                    match &node.stylesheet {
                        Some(sheet) => result.push_str(&sheet.source()),
                        None if node.parent.is_some_and(|parent| {
                            matches!(
                                self.nodes[parent].tag.as_str(),
                                "script"
                                    | "style"
                                    | "xmp"
                                    | "iframe"
                                    | "noembed"
                                    | "noframes"
                                    | "noscript"
                                    | "plaintext"
                            )
                        }) =>
                        {
                            result.push_str(&node.text)
                        }
                        None => result.push_str(&escape(&node.text)),
                    }
                    continue;
                }
                "#comment" => {
                    if svg.is_none() {
                        result.push_str("<!--");
                        result.push_str(&node.text);
                        result.push_str("-->");
                    }
                    continue;
                }
                "#doctype" => {
                    let mut ids = node.text.split('\0');
                    result.push_str(&format!("<!DOCTYPE {}", ids.next().unwrap_or("")));
                    if let (Some(public), Some(system)) = (ids.next(), ids.next()) {
                        result.push_str(&format!(" PUBLIC \"{public}\" \"{system}\""));
                    }
                    result.push('>');
                    continue;
                }
                _ => {}
            }
            let element = id != 0 && !node.tag.starts_with('#');
            if element {
                result.push('<');
                result.push_str(&node.tag);
                let replaced = svg.map(|attrs| attrs(id));
                for (key, value) in &node.attrs {
                    if replaced.as_ref().is_some_and(|replaced| {
                        key == "style" || replaced.iter().any(|(name, _)| name == key)
                    }) {
                        continue;
                    }
                    result.push(' ');
                    result.push_str(key);
                    result.push_str("=\"");
                    result.push_str(&escape(value));
                    result.push('"');
                }
                if mark && !node.attrs.contains_key("id") {
                    result.push_str(&format!(" id=\"apps-{id}\""));
                }
                for (name, value) in replaced.unwrap_or_default() {
                    if !value.is_empty() {
                        result.push(' ');
                        result.push_str(name);
                        result.push_str("=\"");
                        result.push_str(&escape(&value));
                        result.push('"');
                    }
                }
                if node.sf_symbol().is_some() {
                    result.push_str("/>");
                    continue;
                }
                result.push('>');
            }
            if matches!(node.tag.as_str(), "script" | "style") {
                if let Some(sheet) = &node.stylesheet {
                    result.push_str(&sheet.source());
                } else {
                    result.push_str(&node.text);
                }
            } else {
                result.push_str(&escape(&node.text));
            }
            if element
                && !matches!(
                    node.tag.as_str(),
                    "area"
                        | "base"
                        | "br"
                        | "col"
                        | "embed"
                        | "hr"
                        | "img"
                        | "input"
                        | "link"
                        | "meta"
                        | "param"
                        | "source"
                        | "track"
                        | "wbr"
                )
            {
                pending.push((id, true));
            }
            pending.extend(node.children.iter().rev().map(|&child| (child, false)));
        }
        result
    }

    pub fn matches(&self, id: usize, selector: &str) -> bool {
        Selector::parse(selector, None)
            .iter()
            .any(|selector| selector.matches(self, id))
    }
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn decode(value: &str, attribute: bool) -> String {
    const C1: [u16; 32] = [
        0x20ac, 0, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
        0x152, 0, 0x17d, 0, 0, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc,
        0x2122, 0x161, 0x203a, 0x153, 0, 0x17e, 0x178,
    ];
    let entity = |name: &str| -> Option<&'static str> {
        let data = super::entities::ENTITIES;
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
                    super::entities::LEGACY
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

#[cfg(test)]
mod tests {
    use super::*;

    fn find(doc: &Document, tag: &str) -> usize {
        doc.nodes.iter().position(|node| node.tag == tag).unwrap()
    }

    fn parent(doc: &Document, tag: &str) -> String {
        doc.nodes[doc.nodes[find(doc, tag)].parent.unwrap()]
            .tag
            .clone()
    }

    fn tree(doc: &Document, id: usize) -> String {
        let node = &doc.nodes[id];
        match node.tag.as_str() {
            "#text" => format!("{:?}", node.text),
            "#comment" => format!("<!--{}-->", node.text),
            "#doctype" => format!("<!DOCTYPE {}>", node.text.replace('\0', "|")),
            tag => {
                let attrs: String = node
                    .attrs
                    .iter()
                    .map(|(key, value)| format!(" {key}={value:?}"))
                    .collect();
                let children: String = node
                    .children
                    .iter()
                    .map(|&child| tree(doc, child))
                    .collect();
                if tag.starts_with('#') {
                    children
                } else {
                    format!("<{tag}{attrs}>{children}</{tag}>")
                }
            }
        }
    }

    #[test]
    fn sf_symbols_are_void_html_elements_and_roundtrip() {
        for symbol in [
            "<sf-symbol-folder/>",
            "<sf-symbol-folder />",
            "<sf-symbol-folder>",
            "<SF-SYMBOL-FOLDER></SF-SYMBOL-FOLDER>",
        ] {
            let source = format!(
                "<button>{symbol}<span>Open</span><sf-symbol-doc-text class='size-4'/><b>End</b></button>"
            );
            for doc in [Document::parse(&source), Document::fragment(&source, "body")] {
                let button = find(&doc, "button");
                assert_eq!(
                    doc.nodes[button]
                        .children
                        .iter()
                        .map(|&id| doc.nodes[id].tag.as_str())
                        .collect::<Vec<_>>(),
                    ["sf-symbol-folder", "span", "sf-symbol-doc-text", "b"]
                );
                assert!(doc.nodes[find(&doc, "sf-symbol-folder")].children.is_empty());
                assert!(doc.matches(find(&doc, "sf-symbol-folder"), ":-internal-sf-symbol"));
                assert!(!doc.matches(find(&doc, "span"), ":-internal-sf-symbol"));
                let source = doc.serialize(button);
                assert_eq!(
                    source,
                    "<button><sf-symbol-folder/><span>Open</span><sf-symbol-doc-text class=\"size-4\"/><b>End</b></button>"
                );
                let again = Document::parse(&source);
                assert_eq!(tree(&doc, button), tree(&again, find(&again, "button")));
            }
        }
        let doc = Document::parse("<x-card/><span>inside</span></x-card><sf-symbol-folder--fill/><b>inside</b></sf-symbol-folder--fill><i>after</i>");
        assert_eq!(parent(&doc, "span"), "x-card");
        assert_eq!(parent(&doc, "b"), "sf-symbol-folder--fill");
        assert_eq!(parent(&doc, "i"), "body");
        let doc = Document::parse("<svg><sf-symbol-folder/><path d='M0 0'/></svg>");
        assert_eq!(tree(&Document::parse(&doc.serialize(0)), 0), tree(&doc, 0));
    }

    #[test]
    fn closing_tags_do_not_cross_table_scope() {
        let doc = Document::parse(
            "<div><table><tr><td></div><span>inside</span></td></tr></table></div>",
        );
        assert_eq!(parent(&doc, "span"), "td");
    }

    #[test]
    fn unmatched_end_tags_are_linear() {
        let started = std::time::Instant::now();
        for source in [
            format!("{}{}", "<div>".repeat(50000), "</x>".repeat(50000)),
            format!(
                "<div><table>{}{}",
                "<span>".repeat(50000),
                "</div>".repeat(50000)
            ),
            format!(
                "<p><button>{}{}",
                "<span>".repeat(50000),
                "<div>".repeat(50000)
            ),
            format!(
                "<table><tr><td>{}{}",
                "<b>".repeat(50000),
                "</tr>".repeat(50000)
            ),
            format!("<svg>{}{}", "<g>".repeat(50000), "</x>".repeat(50000)),
        ] {
            assert!(Document::parse(&source).nodes.len() > 50000);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let doc = Document::parse(
            "<div><b>x</div>y<svg><linearGradient><stop/></LINEARGRADIENT><g></g></svg><table><tr><td><p>in</table><i>out</i>",
        );
        let y = doc.nodes.iter().position(|n| n.text == "y").unwrap();
        let b = doc.nodes[y].parent.unwrap();
        assert_eq!(
            (doc.nodes[b].tag.as_str(), parent(&doc, "div").as_str()),
            ("b", "body")
        );
        assert_eq!(doc.nodes[doc.nodes[b].parent.unwrap()].tag, "body");
        assert_eq!(parent(&doc, "g"), "svg");
        assert_eq!(parent(&doc, "stop"), "linearGradient");
        assert_eq!(parent(&doc, "i"), "b");
    }

    #[test]
    fn raw_text_and_attributes() {
        let doc = Document::parse(
            "<!doctype html><html><body><h1 id='g' data-x=\"a>b\">你好 &amp; &#65;</h1><script>if (a < b) x = '<div>';</script></body></html>",
        );
        let h = find(&doc, "h1");
        assert_eq!(doc.text(h), "你好 & A");
        assert_eq!(doc.nodes[h].attrs["data-x"], "a>b");
        assert!(doc.matches(h, "body #g"));
        let script = find(&doc, "script");
        assert!(doc.nodes[script].text.is_empty());
        assert_eq!(doc.nodes[script].children.len(), 1);
        assert_eq!(doc.text(script), "if (a < b) x = '<div>';");
        assert_eq!(Document::parse(&doc.serialize(0)).text(h), doc.text(h));
        let doc = Document::parse(
            "<title>a &amp; <b></title><style>a>b{}</style><textarea>\n<i>&lt;</textarea><xmp><p></xmp><noscript><p></noscript><style></style>",
        );
        assert_eq!(
            tree(&doc, 0),
            r#"<html><head><title>"a & <b>"</title><style>"a>b{}"</style></head><body><textarea>"<i><"</textarea><xmp>"<p>"</xmp><noscript>"<p>"</noscript><style></style></body></html>"#
        );
        let style = find(&doc, "style");
        assert_eq!(doc.serialize(style), "<style>a>b{}</style>");
        assert_eq!(
            Document::parse("<p>x <a < b</p>").serialize(0),
            "<html><head></head><body><p>x <a <=\"\" b<=\"\" p=\"\"></a></p></body></html>"
        );
        let doc = Document::parse("<p>a < b &c d<</p>");
        assert_eq!(doc.nodes[find(&doc, "p")].children.len(), 1);
        assert_eq!(doc.text(find(&doc, "p")), "a < b &c d<");
    }

    #[test]
    fn character_references_follow_the_standard_table() {
        for (source, text) in [
            (
                "&notin; &NotEqualTilde; &amp &ampx &copy2023 &notit; &fjlig;",
                "∉ \u{2242}\u{338} & &x ©2023 ¬it; fj",
            ),
            (
                "&#x80;&#150;&#0;&#xD800;&#x110000;&#99999999999;&#x41&#65x",
                "€–\u{fffd}\u{fffd}\u{fffd}\u{fffd}AAx",
            ),
            (
                "&#; &#x; &; &unknown; &AMP; &CounterClockwiseContourIntegral;",
                "&#; &#x; &; &unknown; & ∳",
            ),
            ("a&ltb&gt;&nbsp;&NewLine;&Tab;", "a<b>\u{a0}\n\t"),
        ] {
            assert_eq!(decode(source, false), text, "{source}");
        }
        assert_eq!(decode("?a=1&copy=2&amp;b&lt", true), "?a=1&copy=2&b<");
        assert_eq!(decode("&notit &not &amp;x", true), "&notit ¬ &x");
        let doc =
            Document::parse("<a href='?x=1&lang=en&amp;y' title=&quot;q&quot;>&lt;&#x26;</a>");
        let a = find(&doc, "a");
        assert_eq!(doc.nodes[a].attrs["href"], "?x=1&lang=en&y");
        assert_eq!(doc.nodes[a].attrs["title"], "\"q\"");
        assert_eq!(doc.text(a), "<&");
    }

    #[test]
    fn implied_end_tags_and_document_structure() {
        for (source, expected) in [
            (
                "<!doctype html><title>x</title><p>",
                "<!DOCTYPE html><html><head><title>\"x\"</title></head><body><p></p></body></html>",
            ),
            (
                "<html><body>a",
                "<html><head></head><body>\"a\"</body></html>",
            ),
            (
                "<meta charset=utf-8>\n<link rel=x><body class=a><p>",
                "<html><head><meta charset=\"utf-8\"></meta>\"\\n\"<link rel=\"x\"></link></head><body class=\"a\"><p></p></body></html>",
            ),
            (
                "<head></head>\n<style>s</style><div>",
                "<html><head><style>\"s\"</style></head>\"\\n\"<body><div></div></body></html>",
            ),
            (
                "<p>a<div>b</div><p>c<ul><li>1<li>2<ol><li>3</ol></ul>",
                "<html><head></head><body><p>\"a\"</p><div>\"b\"</div><p>\"c\"</p><ul><li>\"1\"</li><li>\"2\"<ol><li>\"3\"</li></ol></li></ul></body></html>",
            ),
            (
                "<dl><dt>a<dd>b<dt>c</dl><p>x<h1>y</p>z",
                "<html><head></head><body><dl><dt>\"a\"</dt><dd>\"b\"</dd><dt>\"c\"</dt></dl><p>\"x\"</p><h1>\"y\"<p></p>\"z\"</h1></body></html>",
            ),
            (
                "</p><select><option>a<option>b<optgroup><option>c</select>",
                "<html><head></head><body><select><option>\"a\"</option><option>\"b\"</option><optgroup><option>\"c\"</option></optgroup></select></body></html>",
            ),
            (
                "<ruby>a<rb>b<rt>c<rp>d</ruby><pre>\nx</pre>",
                "<html><head></head><body><ruby>\"a\"<rb>\"b\"</rb><rt>\"c\"</rt><rp>\"d\"</rp></ruby><pre>\"x\"</pre></body></html>",
            ),
            (
                "<span><div>x</span>y</div>z",
                "<html><head></head><body><span><div>\"xy\"</div>\"z\"</span></body></html>",
            ),
            (
                "<a href=1>x<a href=2>y</a><br></br><!--c-->",
                "<html><head></head><body><a href=\"1\">\"x\"</a><a href=\"2\">\"y\"</a><br></br><br></br><!--c--></body></html>",
            ),
            (
                "<b>1<p>2</b>3</p>",
                "<html><head></head><body><b>\"1\"</b><p><b>\"2\"</b>\"3\"</p></body></html>",
            ),
            (
                "<a><p>X<a>Y</a>Z</p></a>",
                "<html><head></head><body><a></a><p><a>\"X\"</a><a>\"Y\"</a>\"Z\"</p></body></html>",
            ),
            (
                "<p><b><i>x</p>y",
                "<html><head></head><body><p><b><i>\"x\"</i></b></p><b><i>\"y\"</i></b></body></html>",
            ),
            (
                "<b><div><i>x</b>y</i>z",
                "<html><head></head><body><b></b><div><b><i>\"x\"</i></b><i>\"y\"</i>\"z\"</div></body></html>",
            ),
            (
                "<table><tr><td><b>x</td><td>y</td></tr></table>z",
                "<html><head></head><body><table><tbody><tr><td><b>\"x\"</b></td><td>\"y\"</td></tr></tbody></table>\"z\"</body></html>",
            ),
        ] {
            assert_eq!(tree(&Document::parse(source), 0), expected, "{source}");
        }
        let doc = Document::parse(
            "<!-- a --><!doctype html><?xml x?></3><html lang=en><body></body></html>",
        );
        assert_eq!(
            tree(&doc, 0),
            "<!-- a --><!DOCTYPE html><!--?xml x?--><!--3--><html lang=\"en\"><head></head><body></body></html>"
        );
        for (source, expected) in [
            (
                "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\" 'http://www.w3.org/TR/html4/strict.dtd'><p>",
                "<!DOCTYPE html|-//W3C//DTD HTML 4.01//EN|http://www.w3.org/TR/html4/strict.dtd>",
            ),
            (
                "<!doctype html SYSTEM \"about:legacy-compat\">",
                "<!DOCTYPE html||about:legacy-compat>",
            ),
            ("<!doctype>", "<!DOCTYPE >"),
            ("<html><!doctype html>", ""),
        ] {
            let doc = Document::parse(source);
            let first = &doc.nodes[doc.nodes[0].children[0]];
            assert_eq!(
                if first.tag == "#doctype" {
                    tree(&doc, doc.nodes[0].children[0])
                } else {
                    String::new()
                },
                expected,
                "{source}"
            );
        }
        assert!(
            Document::fragment("<!doctype html><p>", "body")
                .nodes
                .iter()
                .all(|node| node.tag != "#doctype")
        );
        let legacy = Document::parse("<!DOCTYPE html PUBLIC \"a\" \"b\"><p>x");
        assert!(
            legacy
                .serialize(0)
                .starts_with("<!DOCTYPE html PUBLIC \"a\" \"b\"><html>")
        );
        assert_eq!(
            Document::parse(&legacy.serialize(0)).nodes[1].text,
            "html\0a\0b"
        );
    }

    #[test]
    fn tables_imply_sections_rows_and_foster_parent() {
        for (source, expected) in [
            (
                "<table><tr><td>a<td>b<tr><th>c</table>",
                "<table><tbody><tr><td>\"a\"</td><td>\"b\"</td></tr><tr><th>\"c\"</th></tr></tbody></table>",
            ),
            (
                "<table><td>a</td></table>",
                "<table><tbody><tr><td>\"a\"</td></tr></tbody></table>",
            ),
            (
                "<table><thead><tr><th>h<tbody><tr><td>b<tfoot><tr><td>f</table>",
                "<table><thead><tr><th>\"h\"</th></tr></thead><tbody><tr><td>\"b\"</td></tr></tbody><tfoot><tr><td>\"f\"</td></tr></tfoot></table>",
            ),
            (
                "<table><caption>c<tr><td>x</table>",
                "<table><caption>\"c\"</caption><tbody><tr><td>\"x\"</td></tr></tbody></table>",
            ),
            (
                "<table><col><col></table>",
                "<table><colgroup><col></col><col></col></colgroup></table>",
            ),
            (
                "<table>\n<tr>\n<td>a</td>\n</tr>\n</table>",
                "<table>\"\\n\"<tbody><tr>\"\\n\"<td>\"a\"</td>\"\\n\"</tr>\"\\n\"</tbody></table>",
            ),
            (
                "<table>x<b>y</b><tr><td>z</table>",
                "\"x\"<b>\"y\"</b><table><tbody><tr><td>\"z\"</td></tr></tbody></table>",
            ),
            (
                "<table><tr><td><table><tr><td>in</table>out</td></tr></table>",
                "<table><tbody><tr><td><table><tbody><tr><td>\"in\"</td></tr></tbody></table>\"out\"</td></tr></tbody></table>",
            ),
            (
                "<table><tr><td>a<p>b</table><i>c",
                "<table><tbody><tr><td>\"a\"<p>\"b\"</p></td></tr></tbody></table><i>\"c\"</i>",
            ),
            (
                "<table><input type=hidden><select><option>a</table>",
                "<select><option>\"a\"</option></select><table><input type=\"hidden\"></input></table>",
            ),
        ] {
            let doc = Document::fragment(source, "body");
            assert_eq!(tree(&doc, 0), expected, "{source}");
        }
        let doc = Document::fragment("<table><select><option>a<td>b</table>", "body");
        assert_eq!(
            tree(&doc, 0),
            "<select><option>\"a\"</option></select><table><tbody><tr><td>\"b\"</td></tr></tbody></table>"
        );
    }

    #[test]
    fn fragments_follow_their_context() {
        for (context, source, expected) in [
            ("tbody", "<tr><td>a</td></tr>", "<tr><td>\"a\"</td></tr>"),
            ("tr", "<td>a<td>b", "<td>\"a\"</td><td>\"b\"</td>"),
            (
                "table",
                "<tr><td>a",
                "<tbody><tr><td>\"a\"</td></tr></tbody>",
            ),
            (
                "select",
                "<option>a<option>b<div>c</div>",
                "<option>\"a\"</option><option>\"bc\"</option>",
            ),
            ("template", "<tr><td>a</td></tr>", "<tr><td>\"a\"</td></tr>"),
            (
                "template",
                "<td>a</td><td>b",
                "<td>\"a\"</td><td>\"b\"</td>",
            ),
            ("body", "<tr><td>a</td></tr>x", "\"ax\""),
            ("div", "<p>a<p>b", "<p>\"a\"</p><p>\"b\"</p>"),
            ("style", "a>b{}<p>&amp;", "\"a>b{}<p>&amp;\""),
            ("textarea", "<b>&amp;", "\"<b>&\""),
            (
                "svg",
                "<lineargradient gradientunits='x'/><div>y</div>",
                "<linearGradient gradientUnits=\"x\"></linearGradient><div>\"y\"</div>",
            ),
            (
                "html",
                "<head><title>t</title></head><body>b",
                "<head><title>\"t\"</title></head><body>\"b\"</body>",
            ),
            ("ul", "<li>a<li>b", "<li>\"a\"</li><li>\"b\"</li>"),
        ] {
            assert_eq!(
                tree(&Document::fragment(source, context), 0),
                expected,
                "{context}: {source}"
            );
        }
        assert!(Document::fragment("", "style").nodes[0].children.is_empty());
        assert_eq!(
            Document::fragment("<template><tr><td>x</td></tr></template>", "body")
                .nodes
                .len(),
            5
        );
    }

    #[test]
    fn svg_adjusts_case_and_integration_points() {
        let doc = Document::parse(
            r#"<svg VIEWBOX="0 0 16 16" xlink:href=x><defs><LINEARGRADIENT id="paint" GRADIENTUNITS="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/><foreignObject><div><p>html</div><svg><clippath/></svg></foreignObject><title>a<b>t</b></title><style>.a>b{}</style><![CDATA[<x>]]></svg><p>after"#,
        );
        let svg = find(&doc, "svg");
        assert_eq!(doc.nodes[svg].attrs["viewBox"], "0 0 16 16");
        assert_eq!(doc.nodes[svg].attrs["xlink:href"], "x");
        let gradient = find(&doc, "linearGradient");
        assert_eq!(doc.nodes[gradient].attrs["gradientUnits"], "userSpaceOnUse");
        assert!(doc.nodes[gradient].children.is_empty());
        assert_eq!(parent(&doc, "path"), "svg");
        assert_eq!(parent(&doc, "div"), "foreignObject");
        assert_eq!(parent(&doc, "clipPath"), "svg");
        assert_eq!(parent(&doc, "b"), "title");
        assert_eq!(doc.text(find(&doc, "style")), ".a>b{}");
        assert!(
            doc.nodes
                .iter()
                .any(|node| node.tag == "#text" && node.text == "<x>")
        );
        assert_eq!(parent(&doc, "p"), "div");
        let after = doc
            .nodes
            .iter()
            .position(|node| node.text == "after")
            .unwrap();
        assert_eq!(doc.nodes[doc.nodes[after].parent.unwrap()].tag, "p");
        assert_eq!(
            doc.nodes[doc.nodes[doc.nodes[after].parent.unwrap()].parent.unwrap()].tag,
            "body"
        );
        let doc = Document::fragment("<svg><g><div>x</div></g><circle/></svg>", "body");
        assert_eq!(
            tree(&doc, 0),
            "<svg><g></g></svg><div>\"x\"</div><circle></circle>"
        );
    }

    #[test]
    fn html_keeps_comments_and_svg_sources_drop_them() {
        let doc = Document::parse(
            "<p>a<!-- -->b<svg><!--x--><g style='fill:red' x='1' transform='scale(2)'></g></svg></p>",
        );
        assert!(doc.serialize(0).contains("<p>a<!-- -->b<svg><!--x--><g style=\"fill:red\" x=\"1\" transform=\"scale(2)\"></g></svg></p>"));
        let svg = find(&doc, "svg");
        assert_eq!(
            doc.serialize_svg(svg, false, &|node| if doc.nodes[node].tag == "g" {
                vec![
                    ("style", "fill:blue".into()),
                    ("x", "2".into()),
                    ("transform", String::new()),
                ]
            } else {
                Vec::new()
            }),
            "<svg><g style=\"fill:blue\" x=\"2\"></g></svg>"
        );
    }

    #[test]
    fn svg_keeps_case_and_self_closing_siblings() {
        let doc = Document::parse(
            r#"<svg viewBox="0 0 16 16"><defs><linearGradient id="paint" gradientUnits="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/></svg>"#,
        );
        let svg = find(&doc, "svg");
        assert_eq!(doc.nodes[svg].attrs["viewBox"], "0 0 16 16");
        assert_eq!(doc.nodes[svg].children, vec![svg + 1, svg + 3, svg + 4]);
        assert_eq!(doc.nodes[svg + 2].tag, "linearGradient");
        assert_eq!(doc.nodes[svg + 2].attrs["gradientUnits"], "userSpaceOnUse");
        let source = doc.serialize(svg);
        let tree = usvg::Tree::from_str(&source, &Default::default()).unwrap();
        assert_eq!(tree.root().children().len(), 2);
    }

    #[test]
    fn only_edits_keep_the_tree_revision() {
        let mut doc = Document::parse("<p>a</p>");
        let (tree, latest) = (doc.nodes.tree(), doc.nodes.latest());
        doc.nodes.edit(1).unwrap().state = 1;
        assert_eq!(doc.nodes.tree(), tree);
        assert_ne!(doc.nodes.latest(), latest);
        doc.nodes[1].state = 2;
        assert_ne!(doc.nodes.tree(), tree);
        let tree = doc.nodes.tree();
        doc.nodes.get_mut(1).unwrap().state = 3;
        assert_ne!(doc.nodes.tree(), tree);
        let tree = doc.nodes.tree();
        doc.nodes.iter_mut().for_each(|node| node.state = 0);
        assert_ne!(doc.nodes.tree(), tree);
    }

    #[test]
    fn every_mutable_access_takes_a_new_revision() {
        let mut doc = Document::parse("<p>a</p><p>b</p>");
        let revisions = |doc: &Document| -> Vec<u64> {
            (0..doc.nodes.len())
                .map(|id| doc.nodes.revision(id))
                .collect()
        };
        let before = revisions(&doc);
        assert!(
            revisions(&Document::parse("<p>a</p><p>b</p>"))
                .iter()
                .all(|revision| !before.contains(revision))
        );
        assert_eq!(doc.nodes[4].tag, "p");
        assert_eq!(revisions(&doc.clone()), before);
        doc.nodes[1].attrs.set("class", "x");
        doc.nodes.get_mut(3).unwrap().state = 1;
        let after = revisions(&doc);
        assert_eq!(
            (0..after.len())
                .filter(|&id| after[id] != before[id])
                .collect::<Vec<_>>(),
            [1, 3]
        );
        doc.nodes.push(Node::default());
        for node in &mut doc.nodes {
            node.text.clear();
        }
        let all = revisions(&doc);
        assert!(all.iter().zip(&after).all(|(new, old)| new > old));
        let copy: Document = serde_json::from_value(serde_json::to_value(&doc).unwrap()).unwrap();
        assert_eq!(copy.nodes.len(), doc.nodes.len());
        assert!(
            revisions(&copy)
                .iter()
                .all(|revision| !all.contains(revision))
        );
    }

    #[test]
    fn hostile_markup_never_panics() {
        for source in [
            "<",
            "</",
            "<!",
            "<!-",
            "<!--",
            "<?",
            "<a",
            "<a b",
            "<a b=",
            "<a b='",
            "</a",
            "</a b='x",
            "&",
            "&#",
            "&#x",
            "&#xffffffffffffffffff;",
            "<table><caption><table>",
            "<select><select><table>",
            "<svg><foreignObject><svg><desc><table><td><svg><title>",
            "<template><template><tr>",
            "<table><template><td></template></table>",
            "<frameset><frame>",
            "é<b é=é>é</b é>",
            "<plaintext><b>",
            "<textarea>",
            "<title>&am",
            "<style></st",
            "<html><html><head><head><body><body>",
            "<colgroup><col>x</colgroup>",
            "</br></p></li></td></table></select></template></svg>",
        ] {
            for context in [
                None,
                Some("body"),
                Some("tr"),
                Some("svg"),
                Some("select"),
                Some("colgroup"),
                Some("html"),
            ] {
                let doc = match context {
                    Some(context) => Document::fragment(source, context),
                    None => Document::parse(source),
                };
                for (id, node) in doc.nodes.iter().enumerate() {
                    for &child in &node.children {
                        assert_eq!(doc.nodes[child].parent, Some(id), "{source}");
                    }
                }
            }
        }
        assert_eq!(
            tree(&Document::parse("<plaintext><b>&amp;"), 0),
            "<html><head></head><body><plaintext>\"<b>&amp;\"</plaintext></body></html>"
        );
        assert_eq!(
            tree(&Document::fragment("a\r\nb\rc", "body"), 0),
            "\"a\\nb\\nc\""
        );
    }
}
