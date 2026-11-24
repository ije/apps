use super::*;
use super::{css::css, emit::bundle};

thread_local! {
    static DOWNLOADS: RefCell<HashMap<String, String>> = RefCell::default();
}

pub(super) fn is_remote(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| path.starts_with("https://") || path.starts_with("http://"))
}

pub(super) fn remote_base(directory: &Path) -> Option<url::Url> {
    is_remote(directory)
        .then(|| url::Url::parse(&format!("{}/", directory.to_str()?)).ok())
        .flatten()
}

pub(super) fn remote(
    directory: &Path,
    reference: &str,
    http: bool,
) -> Result<Option<(PathBuf, String)>> {
    let mut url = match remote_base(directory) {
        Some(base) => {
            ensure!(
                reference.contains(':') || reference.starts_with(['.', '/']),
                "Bare import {reference:?} is not supported in remote files: {base}"
            );
            base.join(reference)?
        }
        None if reference.contains("://") || reference.starts_with("//") => {
            url::Url::parse(reference).with_context(|| format!("Invalid URL: {reference}"))?
        }
        None => return Ok(None),
    };
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Only local files and HTTP URLs can be imported: {url}"
    );
    ensure!(
        http,
        "Importing {url} needs the httpImport feature. Add \"httpImport\" to \"features\" in package.json"
    );
    url.set_fragment(None);
    let source = match DOWNLOADS.with_borrow(|downloads| downloads.get(url.as_str()).cloned()) {
        Some(source) => source,
        None => {
            let source = String::from_utf8(apps_runtime::download(url.as_str())?)
                .with_context(|| format!("{url} is not UTF-8"))?;
            DOWNLOADS
                .with_borrow_mut(|downloads| downloads.insert(url.to_string(), source.clone()));
            source
        }
    };
    let mut path = url.to_string();
    // Keep an empty fragment on directory URLs: `parent()` removes the trailing slash.
    if url.path().ends_with('/') && url.query().is_none() {
        path.push('#');
    }
    Ok(Some((PathBuf::from(path), source)))
}

pub(super) fn urls(source: &str, directory: &Path, shared: &Shared) -> String {
    let bytes = source.as_bytes();
    let string = |start: usize| {
        let mut end = start + 1;
        while end < bytes.len() && bytes[end] != bytes[start] && bytes[end] != b'\n' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        (end + 1).min(bytes.len())
    };
    let (mut result, mut copied, mut pos) = (String::with_capacity(source.len()), 0, 0);
    while pos < bytes.len() {
        if bytes[pos..].starts_with(b"/*") {
            pos = source[pos + 2..]
                .find("*/")
                .map_or(bytes.len(), |end| pos + end + 4);
        } else if matches!(bytes[pos], b'"' | b'\'') {
            pos = string(pos);
        } else if bytes[pos..].len() >= 4 && bytes[pos..pos + 4].eq_ignore_ascii_case(b"url(") {
            let start = pos + 4;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_whitespace() {
                end += 1;
            }
            if end < bytes.len() && matches!(bytes[end], b'"' | b'\'') {
                end = string(end);
            }
            pos = source[end..]
                .find(')')
                .map_or(bytes.len(), |close| end + close);
            if let Some(url) = shared.asset(directory, &source[start..pos]) {
                result.push_str(&source[copied..start]);
                result.push('"');
                result.push_str(&url);
                result.push('"');
                copied = pos;
            }
        } else {
            pos += 1;
        }
    }
    result.push_str(&source[copied..]);
    result
}

pub(super) fn html(path: &Path, shared: &Rc<Shared>) -> Result<String> {
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
                    shared.watch(file);
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
        let code = script_source(
            &BBTreeMap::from([("app.js".into(), bundle(entries, realm, shared.clone())?)]),
            true,
        );
        let chunk = shared.script(code);
        doc.nodes[first]
            .attrs
            .insert("src".into(), format!("apps:///script-{chunk}.js"));
    }
    // The runtime parses this text again, and the parser drops one newline at the start of these elements.
    let starts: Vec<usize> = (0..doc.nodes.len())
        .filter(|&id| matches!(doc.nodes[id].tag.as_str(), "pre" | "textarea" | "listing"))
        .filter_map(|id| doc.nodes[id].children.first().copied())
        .filter(|&child| doc.nodes[child].tag == "#text" && doc.nodes[child].text.starts_with('\n'))
        .collect();
    for child in starts {
        doc.nodes[child].text.insert(0, '\n');
    }
    Ok(doc.serialize(0))
}
