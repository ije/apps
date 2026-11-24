use super::{CHECKED, DIRTY, editable_value};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

// The elements whose text is raw: the parser reads no tags in it and the serializer escapes nothing.
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

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter().map(|(key, value)| (key, value))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    // Adds an attribute that the caller knows is new.
    pub(crate) fn append(&mut self, name: String, value: String) {
        self.0.push((name, value));
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

impl FromIterator<(String, String)> for Attrs {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(items: T) -> Self {
        Self(items.into_iter().collect())
    }
}

impl IntoIterator for Attrs {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
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
    pub(crate) stylesheet: Option<Rc<crate::css::sheet::Stylesheet>>,
    // Clear this cache on every style attribute write.
    pub(crate) inline: Option<crate::css::sheet::Declarations>,
    #[serde(skip)]
    pub(crate) state: u16,
    #[serde(skip)]
    pub(crate) value: Option<String>,
}

impl Node {
    pub(crate) fn input_type(&self) -> &str {
        self.attrs.get("type").map_or("text", String::as_str)
    }

    /// The AppKit control of an input with `appearance: system`, on macOS.
    pub(crate) fn native_control(
        &self,
        style: &crate::css::properties::Style,
    ) -> Option<renderer::ControlKind> {
        if style.get(crate::css::properties::Prop::APPEARANCE) != Some("system") {
            return None;
        }
        self.system_control()
    }

    /// The AppKit control that `appearance: system` gives this element, on macOS.
    pub(crate) fn system_control(&self) -> Option<renderer::ControlKind> {
        if !cfg!(target_os = "macos") || self.tag != "input" {
            return None;
        }
        Some(match self.input_type().to_ascii_lowercase().as_str() {
            "button" | "submit" | "reset" => renderer::ControlKind::Button,
            "checkbox" if self.attrs.contains_key("switch") => renderer::ControlKind::Switch,
            "range" => renderer::ControlKind::Slider,
            "number" => renderer::ControlKind::Stepper,
            "text" | "email" | "url" | "tel" => renderer::ControlKind::Text,
            "search" => renderer::ControlKind::Search,
            "color" => renderer::ControlKind::Color,
            "date" => renderer::ControlKind::Date,
            _ => return None,
        })
    }

    pub(crate) fn content_editable(&self) -> Option<bool> {
        self.attrs
            .get("contenteditable")
            .and_then(|value| editable_value(value))
    }

    pub(crate) fn checked(&self) -> bool {
        if self.state & DIRTY != 0 {
            self.state & CHECKED != 0
        } else {
            self.attrs.contains_key(if self.tag == "option" {
                "selected"
            } else {
                "checked"
            })
        }
    }

    pub(crate) fn password(&self) -> bool {
        self.tag == "input"
            && self
                .attrs
                .get("type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
    }

    pub(crate) fn shown_text(&self, text: String) -> String {
        if self.password() {
            "\u{2022}".repeat(text.chars().count())
        } else {
            text
        }
    }

    pub(crate) fn current_value(&self) -> Option<&str> {
        self.value.as_deref().or_else(|| {
            (self.tag == "input").then(|| self.attrs.get("value").map_or("", String::as_str))
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Document {
    pub nodes: Nodes,
    // The element of the URL fragment, which `:target` matches.
    #[serde(skip)]
    pub(crate) target: Option<usize>,
}

static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn stamp() -> u64 {
    REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Debug, Default)]
pub struct Nodes {
    list: Vec<Node>,
    revisions: Vec<u64>,
    latest: u64,
    tree: u64,
    // The nodes that got a revision after `logged`, with duplicates, so readers need no scan of all nodes.
    log: Vec<u32>,
    logged: u64,
}

impl Nodes {
    // Call this before a node gets its new revision.
    fn note(&mut self, id: usize) {
        if self.log.last() == Some(&(id as u32)) {
            return;
        }
        if self.log.len() == 4096 {
            self.log.clear();
            self.logged = self.latest;
        }
        self.log.push(id as u32);
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.list.truncate(len);
        self.list.shrink_to_fit();
        self.revisions.truncate(len);
        self.revisions.shrink_to_fit();
        self.latest = stamp();
        self.tree = self.latest;
        self.log.clear();
        self.logged = self.latest;
    }

    pub fn push(&mut self, node: Node) {
        self.note(self.list.len());
        self.list.push(node);
        self.latest = stamp();
        self.tree = self.latest;
        self.revisions.push(self.latest);
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Node> {
        self.list.get(id)?;
        self.note(id);
        self.latest = stamp();
        self.tree = self.latest;
        self.revisions[id] = self.latest;
        self.list.get_mut(id)
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Node> {
        self.revisions.fill(stamp());
        self.latest = stamp();
        self.tree = self.latest;
        self.log.clear();
        self.logged = self.latest;
        self.list.iter_mut()
    }

    // Never change parent or children through edit: it does not invalidate cached tree walks.
    pub(crate) fn edit(&mut self, id: usize) -> Option<&mut Node> {
        self.list.get(id)?;
        self.note(id);
        self.latest = stamp();
        self.revisions[id] = self.latest;
        self.list.get_mut(id)
    }

    pub(crate) fn tree(&self) -> u64 {
        self.tree
    }

    // The revision stays, so the frame must put the old children back before it ends.
    pub fn compose_children(&mut self, id: usize, children: Vec<usize>) -> Vec<usize> {
        std::mem::replace(&mut self.list[id].children, children)
    }

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

    // The nodes with a newer revision. A node can come more than once.
    pub(crate) fn changed_since(&self, revision: u64) -> impl Iterator<Item = usize> + '_ {
        let newer = move |&id: &usize| {
            self.revisions
                .get(id)
                .is_some_and(|&stamp| stamp > revision)
        };
        let logged = (revision >= self.logged).then(|| self.log.iter().map(|&id| id as usize));
        let every = (revision < self.logged).then_some(0..self.revisions.len());
        logged
            .into_iter()
            .flatten()
            .chain(every.into_iter().flatten())
            .filter(newer)
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
        self.get_mut(id).unwrap()
    }
}

impl From<Vec<Node>> for Nodes {
    fn from(list: Vec<Node>) -> Self {
        let revisions = vec![stamp(); list.len()];
        let latest = stamp();
        Self {
            list,
            revisions,
            latest,
            tree: latest,
            log: Vec::new(),
            logged: latest,
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

impl Document {
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
        let mut foreign = false;
        let mut up = Some(id);
        while let Some(at) = up {
            let tag = self.nodes[at].tag.as_str();
            if tag == "svg" || at != id && matches!(tag, "foreignObject" | "desc" | "title") {
                foreign = tag == "svg";
                break;
            }
            up = self.nodes[at].parent;
        }
        let mut pending = vec![(id, false, foreign)];
        while let Some((id, close, foreign)) = pending.pop() {
            let node = &self.nodes[id];
            if close {
                result.push_str("</");
                result.push_str(&node.tag);
                result.push('>');
                continue;
            }
            match node.tag.as_str() {
                "#text" => {
                    let source = node.stylesheet.as_ref().map(|sheet| sheet.source());
                    let text = source.as_deref().unwrap_or(&node.text);
                    if !foreign
                        && node
                            .parent
                            .is_some_and(|parent| RAW.contains(&self.nodes[parent].tag.as_str()))
                    {
                        result.push_str(text);
                    } else {
                        result.push_str(&escape(text));
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
                    result.push_str("<!DOCTYPE ");
                    result.push_str(ids.next().unwrap_or(""));
                    if let (Some(public), Some(system)) = (ids.next(), ids.next()) {
                        result.push_str(" PUBLIC \"");
                        result.push_str(public);
                        result.push_str("\" \"");
                        result.push_str(system);
                        result.push('"');
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
                    result.push_str(" id=\"apps-");
                    result.push_str(&id.to_string());
                    result.push('"');
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
                result.push('>');
            }
            if !foreign && matches!(node.tag.as_str(), "script" | "style") {
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
                pending.push((id, true, foreign));
            }
            let inner = node.tag == "svg"
                || foreign && !matches!(node.tag.as_str(), "foreignObject" | "desc" | "title");
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|&child| (child, false, inner)),
            );
        }
        result
    }
}

pub fn escape(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.contains(['&', '<', '>', '"']) {
        return value.into();
    }
    let mut escaped = String::with_capacity(value.len());
    let mut last = 0;
    for (at, found) in value.match_indices(['&', '<', '>', '"']) {
        escaped.push_str(&value[last..at]);
        escaped.push_str(match found {
            "&" => "&amp;",
            "<" => "&lt;",
            ">" => "&gt;",
            _ => "&quot;",
        });
        last = at + 1;
    }
    escaped.push_str(&value[last..]);
    escaped.into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn password_fields_show_a_bullet_per_code_point() {
        let doc = Document::parse("<input type=password><input>");
        let [password, text] = [1, 2].map(|nth| {
            doc.nodes
                .iter()
                .filter(|node| node.tag == "input")
                .nth(nth - 1)
                .unwrap()
        });
        let value = "a\u{1F600}b";
        assert_eq!(password.shown_text(value.into()), "\u{2022}".repeat(3));
        assert_eq!(text.shown_text(value.into()), value);
        assert_eq!(
            [0, 1, 2, 3, 4].map(|offset| crate::dom::text::bullets(value, offset)),
            [0, 1, 2, 2, 3]
        );
        assert_eq!(
            [0, 1, 2, 3].map(|index| crate::dom::text::unbullet(value, index)),
            [0, 1, 3, 4]
        );
    }
}
