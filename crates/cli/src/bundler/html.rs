use super::{
    Entry, Realm, Shared,
    css::{css, urls},
    resolve::remote,
};
use anyhow::{Context, Result};
use apps_runtime::dom::Document;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

// A page, with the groups of scripts that the caller bundles: the realm, the script node that loads the
// chunk, and the entries.
pub(super) struct Page {
    doc: Document,
    groups: Vec<(Realm, usize, Vec<Entry>)>,
}

pub(super) fn html(path: &Path, shared: &Shared) -> Result<Page> {
    let mut doc = Document::parse(&fs::read_to_string(path)?);
    let parent = path.parent().context("Missing HTML directory")?;
    let http = shared.enabled("httpImport");
    let mut blocking = Vec::new();
    let mut scripts: [Vec<(usize, Entry)>; 2] = Default::default();
    for index in 0..doc.nodes.len() {
        let text = matches!(doc.nodes[index].tag.as_str(), "style" | "script")
            .then(|| doc.text(index))
            .unwrap_or_default();
        let child = doc.nodes.len();
        let node = &mut doc.nodes[index];
        if let Some(style) = node.attrs.get_mut("style") {
            *style = urls(style, parent, shared);
        }
        let references: &[&str] = match node.tag.as_str() {
            "img" | "audio" | "source" => &["src"],
            "video" => &["src", "poster"],
            _ => &[],
        };
        for name in references {
            if let Some(source) = node.attrs.get_mut(name)
                && let Some(url) = shared.asset(parent, source)
            {
                *source = url;
            }
        }
        if matches!(node.tag.as_str(), "img" | "source")
            && let Some(srcset) = node.attrs.get_mut("srcset")
        {
            let mut result = String::with_capacity(srcset.len());
            let mut rest = srcset.as_str();
            while let Some(start) = rest.find(|c: char| !c.is_ascii_whitespace() && c != ',') {
                let candidate = &rest[start..];
                let end = candidate
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(candidate.len());
                let url = candidate[..end].trim_end_matches(',');
                result.push_str(&rest[..start]);
                result.push_str(&shared.asset(parent, url).unwrap_or_else(|| url.into()));
                let descriptors = &candidate[url.len()..];
                let next = if url.len() < end {
                    0
                } else {
                    descriptors
                        .find(',')
                        .map_or(descriptors.len(), |comma| comma + 1)
                };
                result.push_str(&descriptors[..next]);
                rest = &descriptors[next..];
            }
            result.push_str(rest);
            *srcset = result;
        }
        let mut sheet = None;
        if node.tag == "style" {
            sheet = Some(css(path, Some(text), &mut Vec::new(), shared)?);
        } else if node.tag == "link"
            && node.attrs.get("rel").is_some_and(|rel| {
                let mut tokens = rel.split_ascii_whitespace();
                tokens
                    .clone()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    && !tokens.any(|token| token.eq_ignore_ascii_case("alternate"))
            })
        {
            let href = node.attrs.get("href").context("Stylesheet has no href")?;
            let text = match remote(parent, href, http)? {
                Some((path, text)) => css(&path, Some(text), &mut Vec::new(), shared)?,
                None => css(&shared.local(parent, href)?, None, &mut Vec::new(), shared)?,
            };
            node.attrs
                .insert(apps_runtime::bundle::LINK_SHEET.into(), text);
        } else if node.tag == "script" {
            let (classic, preload) = match node
                .attrs
                .get("type")
                .map(|kind| kind.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("" | "text/javascript" | "application/javascript") => (true, false),
                Some("module") => (false, false),
                Some("preload") => (false, true),
                _ => continue,
            };
            let src = if preload {
                node.attrs.remove("src")
            } else {
                node.attrs.get("src").cloned()
            };
            node.children.clear();
            if classic && node.attrs.contains_key("nomodule") {
                continue;
            }
            let deferred = src.is_some()
                && (node.attrs.contains_key("defer") || node.attrs.contains_key("async"));
            let entry = match src {
                Some(src) => match remote(parent, &src, http)? {
                    Some((path, source)) => (path, Some(source)),
                    None => (shared.local(parent, &src)?, None),
                },
                None => (path.to_owned(), Some(text)),
            };
            if !classic {
                scripts[usize::from(preload)].push((index, entry));
                continue;
            }
            let source = match entry {
                (_, Some(source)) => source,
                (file, None) => {
                    let source = fs::read_to_string(&file)
                        .with_context(|| format!("Cannot read {}", file.display()))?;
                    shared.files.borrow_mut().insert(file);
                    source
                }
            };
            let chunk = shared.script(source);
            let entry = (PathBuf::from(format!("apps:///script-{chunk}.js")), None);
            if deferred {
                scripts[0].push((index, entry));
            } else {
                blocking.push((index, entry));
            }
        }
        if let Some(sheet) = sheet {
            node.children = vec![child];
            let mut text = apps_runtime::dom::Node::default();
            text.tag = "#text".into();
            text.text = sheet;
            text.parent = Some(index);
            doc.nodes.push(text);
        }
    }
    scripts[0].splice(0..0, blocking);
    if let Some(preload) = shared.preload_import() {
        let parent = doc
            .nodes
            .iter()
            .position(|node| node.tag == "head")
            .unwrap_or(0);
        let index = doc.nodes.len();
        let mut node = apps_runtime::dom::Node::default();
        node.tag = "script".into();
        node.attrs.insert("type".into(), "preload".into());
        node.parent = Some(parent);
        doc.nodes.push(node);
        doc.nodes[parent].children.insert(0, index);
        scripts[1].insert(0, (index, (PathBuf::from("apps:preload"), Some(preload))));
    }
    let mut groups = Vec::new();
    for (realm, list) in [Realm::Renderer, Realm::Preload].into_iter().zip(scripts) {
        let Some(&(mut first, _)) = list.first() else {
            continue;
        };
        let mut order = HashMap::new();
        if realm == Realm::Renderer {
            let parent = doc.nodes[first].parent.unwrap_or(0);
            let mut node = apps_runtime::dom::Node::default();
            node.tag = "script".into();
            node.attrs.insert("type".into(), "module".into());
            node.parent = Some(parent);
            doc.nodes.push(node);
            let at = doc.nodes[parent]
                .children
                .iter()
                .position(|&child| child == first)
                .unwrap_or(0);
            first = doc.nodes.len() - 1;
            doc.nodes[parent].children.insert(at, first);
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                if doc.nodes[id].tag == "script" {
                    order.insert(id, order.len());
                }
                pending.extend(doc.nodes[id].children.iter().rev());
            }
        }
        let entries = list
            .into_iter()
            .map(|(node, (path, inline))| match path.to_str() {
                Some(name) if name.starts_with("apps:///") => {
                    (PathBuf::from(format!("{name}#{}", order[&node])), inline)
                }
                _ => (path, inline),
            })
            .collect();
        groups.push((realm, first, entries));
    }
    Ok(Page { doc, groups })
}

impl Page {
    // Serializes the page, with the script chunk that `bundle` gives each group.
    pub(super) fn finish(
        mut self,
        mut bundle: impl FnMut(Vec<Entry>, Realm) -> Result<usize>,
    ) -> Result<String> {
        for (realm, first, entries) in std::mem::take(&mut self.groups) {
            let chunk = bundle(entries, realm)?;
            self.doc.nodes[first]
                .attrs
                .insert("src".into(), format!("apps:///script-{chunk}.js"));
        }
        // The runtime parses this text again, and the parser drops one newline at the start of these elements.
        let starts: Vec<usize> = (0..self.doc.nodes.len())
            .filter(|&id| {
                matches!(
                    self.doc.nodes[id].tag.as_str(),
                    "pre" | "textarea" | "listing"
                )
            })
            .filter_map(|id| self.doc.nodes[id].children.first().copied())
            .filter(|&child| {
                self.doc.nodes[child].tag == "#text" && self.doc.nodes[child].text.starts_with('\n')
            })
            .collect();
        for child in starts {
            self.doc.nodes[child].text.insert(0, '\n');
        }
        Ok(self.doc.serialize(0))
    }
}
