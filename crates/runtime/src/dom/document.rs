use super::Selector;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

/// Attributes in source order, like the DOM attribute list.
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
    pub(crate) inline: Option<crate::style::Declarations>,
    #[serde(skip)]
    pub(crate) state: u8,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Document {
    pub nodes: Vec<Node>,
}

impl Document {
    pub fn parse(source: &str) -> Self {
        let mut doc = Self {
            nodes: vec![Node {
                tag: "#document".into(),
                ..Node::default()
            }],
        };
        let mut stack = vec![(0, false)];
        let mut pos = 0;
        while pos < source.len() {
            let rest = &source[pos..];
            if rest.starts_with("<!") || rest.starts_with("<?") {
                let (text, length) = if let Some(body) = rest.strip_prefix("<!--") {
                    body.find("-->")
                        .map_or((body, rest.len()), |n| (&body[..n], n + 7))
                } else {
                    let end = rest.find('>').unwrap_or(rest.len());
                    (
                        &rest[if rest.starts_with("<?") { 1 } else { 2 }..end],
                        end + 1,
                    )
                };
                pos += length.min(rest.len());
                if !text
                    .get(..7)
                    .is_some_and(|tag| tag.eq_ignore_ascii_case("doctype"))
                {
                    let (parent, id) = (stack.last().unwrap().0, doc.nodes.len());
                    doc.nodes[parent].children.push(id);
                    doc.nodes.push(Node {
                        tag: "#comment".into(),
                        text: text.into(),
                        parent: Some(parent),
                        ..Node::default()
                    });
                }
                continue;
            }
            if let Some(close) = rest.strip_prefix("</") {
                let end = close.find('>').unwrap_or(close.len());
                let tag = close[..end].trim().to_ascii_lowercase();
                for index in (1..stack.len()).rev() {
                    let node = &doc.nodes[stack[index].0];
                    if node.tag.eq_ignore_ascii_case(&tag) {
                        stack.truncate(index);
                        break;
                    }
                    if matches!(
                        node.tag.as_str(),
                        "html" | "table" | "td" | "th" | "template" | "select"
                    ) {
                        break;
                    }
                }
                pos += (end + 3).min(rest.len());
                continue;
            }
            let (parent, in_svg) = *stack.last().unwrap();
            let mut node = Node {
                parent: Some(parent),
                ..Node::default()
            };
            if rest.starts_with('<') && rest.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            {
                let mut end = 1;
                let mut quote = 0;
                for (i, byte) in rest.bytes().enumerate().skip(1) {
                    if quote != 0 {
                        if byte == quote {
                            quote = 0;
                        }
                    } else if byte == b'\'' || byte == b'"' {
                        quote = byte;
                    } else if byte == b'>' {
                        end = i;
                        break;
                    }
                    end = i + 1;
                }
                let inside = &rest[1..end];
                let name_end = inside
                    .find(|c: char| c.is_ascii_whitespace() || c == '/')
                    .unwrap_or(inside.len());
                node.tag = inside[..name_end].to_ascii_lowercase();
                let svg = in_svg || node.tag == "svg";
                if svg {
                    node.tag = inside[..name_end].to_owned();
                }
                let mut attrs = inside[name_end..].trim();
                while !attrs.is_empty() && attrs != "/" {
                    let split = attrs
                        .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
                        .unwrap_or(attrs.len());
                    if split == 0 {
                        break;
                    }
                    let name = if svg {
                        attrs[..split].to_owned()
                    } else {
                        attrs[..split].to_ascii_lowercase()
                    };
                    attrs = attrs[split..].trim_start();
                    let mut value = "";
                    if let Some(tail) = attrs.strip_prefix('=') {
                        attrs = tail.trim_start();
                        if attrs.starts_with(['\'', '"']) {
                            let quote = attrs.as_bytes()[0] as char;
                            attrs = &attrs[1..];
                            let length = attrs.find(quote).unwrap_or(attrs.len());
                            value = &attrs[..length];
                            attrs = &attrs[(length + 1).min(attrs.len())..];
                        } else {
                            let length = attrs.find(char::is_whitespace).unwrap_or(attrs.len());
                            value = &attrs[..length];
                            attrs = &attrs[length..];
                        }
                    }
                    if !node.attrs.contains_key(&name) {
                        node.attrs.insert(name, decode(value));
                    }
                    attrs = attrs.trim_start();
                }
                pos += (end + 1).min(rest.len());
                let id = doc.nodes.len();
                let void = matches!(
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
                );
                let raw = matches!(node.tag.as_str(), "script" | "style" | "title" | "textarea");
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                if raw {
                    let closing = format!("</{}", doc.nodes[id].tag);
                    let tail = &source[pos..];
                    let length = tail
                        .as_bytes()
                        .windows(closing.len())
                        .enumerate()
                        .find(|(offset, w)| {
                            w.eq_ignore_ascii_case(closing.as_bytes())
                                && tail
                                    .as_bytes()
                                    .get(offset + closing.len())
                                    .is_none_or(|c| c.is_ascii_whitespace() || *c == b'>')
                        })
                        .map(|(offset, _)| offset)
                        .unwrap_or(tail.len());
                    doc.nodes[id].text =
                        if matches!(doc.nodes[id].tag.as_str(), "title" | "textarea") {
                            decode(&tail[..length])
                        } else {
                            tail[..length].into()
                        };
                    pos += length;
                    if let Some(end) = source[pos..].find('>') {
                        pos += end + 1;
                    }
                } else if !void && !(svg && inside.trim_end().ends_with('/')) {
                    stack.push((id, svg));
                }
            } else {
                let first = rest.chars().next().unwrap().len_utf8();
                let length = rest[first..].find('<').map_or(rest.len(), |n| n + first);
                node.tag = "#text".into();
                node.text = decode(&rest[..length]);
                let id = doc.nodes.len();
                doc.nodes.push(node);
                doc.nodes[parent].children.push(id);
                pos += length;
            }
        }
        doc
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
            if id != root && node.tag == "#comment" {
                continue;
            }
            match &node.stylesheet {
                Some(sheet) => text.push_str(&sheet.source()),
                None => text.push_str(&node.text),
            }
            pending.extend(node.children.iter().rev());
        }
        text
    }

    pub fn serialize(&self, id: usize) -> String {
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
                    result.push_str(&escape(&node.text));
                    continue;
                }
                "#comment" => continue,
                _ => {}
            }
            let element = id != 0 && !node.tag.starts_with('#');
            if element {
                result.push('<');
                result.push_str(&node.tag);
                for (key, value) in &node.attrs {
                    result.push(' ');
                    result.push_str(key);
                    result.push_str("=\"");
                    result.push_str(&escape(value));
                    result.push('"');
                }
                if let Some(inline) = &node.inline {
                    result.push_str(" style=\"");
                    result.push_str(&escape(&inline.source()));
                    result.push('"');
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

pub fn decode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|n| *n < 16) else {
            result.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let character = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(character) = character {
            result.push(character);
        } else {
            result.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_tags_do_not_cross_table_scope() {
        let doc = Document::parse(
            "<div><table><tr><td></div><span>inside</span></td></tr></table></div>",
        );
        let span = doc
            .nodes
            .iter()
            .position(|node| node.tag == "span")
            .unwrap();
        assert_eq!(doc.nodes[doc.nodes[span].parent.unwrap()].tag, "td");
    }

    #[test]
    fn raw_text_and_attributes() {
        let doc = Document::parse(
            "<!doctype html><html><body><h1 id='g' data-x=\"a>b\">你好 &amp; &#65;</h1><script>if (a < b) x = '<div>';</script></body></html>",
        );
        let h = doc.nodes.iter().position(|n| n.tag == "h1").unwrap();
        assert_eq!(doc.text(h), "你好 & A");
        assert_eq!(doc.nodes[h].attrs["data-x"], "a>b");
        assert!(doc.matches(h, "body #g"));
        assert!(
            doc.nodes
                .iter()
                .any(|n| n.text == "if (a < b) x = '<div>';")
        );
        assert_eq!(Document::parse(&doc.serialize(0)).text(h), doc.text(h));
    }

    #[test]
    fn svg_keeps_case_and_self_closing_siblings() {
        let doc = Document::parse(
            r#"<svg viewBox="0 0 16 16"><defs><linearGradient id="paint" gradientUnits="userSpaceOnUse"/></defs><path d="M0 0h8v8H0Z"/><path d="M8 8h8v8H8Z"/></svg>"#,
        );
        assert_eq!(doc.nodes[1].attrs["viewBox"], "0 0 16 16");
        assert_eq!(doc.nodes[1].children, vec![2, 4, 5]);
        assert_eq!(doc.nodes[3].tag, "linearGradient");
        assert_eq!(doc.nodes[3].attrs["gradientUnits"], "userSpaceOnUse");
        let source = doc.serialize(1);
        let tree = resvg::usvg::Tree::from_str(&source, &Default::default()).unwrap();
        assert_eq!(tree.root().children().len(), 2);
    }
}
