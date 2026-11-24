use super::*;
use super::{
    html::{remote, urls},
    resolve::exported,
};

pub(super) fn css_package(directory: &Path, name: &str) -> Result<PathBuf> {
    let count = if name.starts_with('@') { 2 } else { 1 };
    let package = name.split('/').take(count).collect::<Vec<_>>().join("/");
    let subpath = name[package.len()..].trim_start_matches('/');
    let root = directory
        .ancestors()
        .map(|directory| directory.join("node_modules").join(&package))
        .find(|path| path.is_dir())
        .with_context(|| format!("Cannot resolve CSS import {name}"))?;
    let metadata: Value = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|source| serde_json::from_str(&source).ok())
        .unwrap_or_default();
    // Like Tailwind's resolver, `exports` uses the `style` condition, and a package without a match falls back
    // to plain paths.
    if let Some(entry) = metadata.get("exports").and_then(|exports| {
        let key = if subpath.is_empty() {
            "."
        } else {
            &format!("./{subpath}")
        };
        exported(exports, key, &["style", "default"], false)
            .ok()
            .filter(|entry| entry.starts_with("./") && entry.ends_with(".css"))
    }) {
        return Ok(root.join(entry));
    }
    if !subpath.is_empty() {
        return Ok(root.join(subpath));
    }
    let entry = metadata["style"]
        .as_str()
        .or_else(|| {
            metadata["main"]
                .as_str()
                .filter(|main| main.ends_with(".css"))
        })
        .unwrap_or("index.css");
    Ok(root.join(entry))
}

pub(super) fn css(
    path: &Path,
    inline: Option<String>,
    stack: &mut Vec<PathBuf>,
    shared: &Shared,
) -> Result<String> {
    ensure!(
        !stack.contains(&path.to_owned()),
        "CSS import cycle: {}",
        path.display()
    );
    stack.push(path.to_owned());
    let source = match inline {
        Some(source) => source,
        None => {
            shared.watch(path.to_owned());
            ensure!(
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("css")),
                "Stylesheets must be .css files: {}",
                path.display()
            );
            fs::read_to_string(path)?
        }
    };
    let source = urls(
        &source,
        path.parent().context("Missing CSS directory")?,
        shared,
    );
    let mut result = String::new();
    let mut rest = source.as_str();
    loop {
        let bytes = rest.as_bytes();
        let mut pos = 0;
        let mut quote = 0;
        let mut depth = 0;
        let mut found = None;
        while pos < bytes.len() {
            let byte = bytes[pos];
            if quote != 0 {
                if byte == b'\\' {
                    pos += 2;
                    continue;
                }
                if byte == quote {
                    quote = 0;
                }
            } else if byte == b'\'' || byte == b'"' {
                quote = byte;
            } else if bytes[pos..].starts_with(b"/*") {
                pos += 2;
                pos += bytes[pos..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(bytes.len() - pos, |end| end + 2);
                continue;
            } else if byte == b'{' {
                depth += 1;
            } else if byte == b'}' {
                depth -= 1;
            } else if depth == 0
                && bytes[pos..]
                    .get(..7)
                    .is_some_and(|name| name.eq_ignore_ascii_case(b"@import"))
            {
                found = Some(pos);
                break;
            }
            pos += 1;
        }
        let Some(start) = found else {
            break;
        };
        result.push_str(&rest[..start]);
        rest = &rest[start + 7..];
        let end = rest.find(';').context("CSS @import needs a semicolon")?;
        let spec = rest[..end].trim();
        let (spec, options) = match spec.strip_prefix("url(") {
            Some(inner) => {
                let close = inner.find(')').context("Expected a closing parenthesis")?;
                (inner[..close].trim(), inner[close + 1..].trim())
            }
            None => match spec.as_bytes().first() {
                Some(&quote @ (b'\'' | b'"')) => {
                    let close = spec[1..]
                        .find(quote as char)
                        .context("Expected a string literal")?;
                    (&spec[..close + 2], spec[close + 2..].trim())
                }
                _ => (spec, ""),
            },
        };
        let spec = if spec.starts_with(['\'', '"']) {
            spec
        } else {
            &format!("'{spec}'")
        };
        ensure!(
            spec.len() >= 2 && spec.starts_with(['\'', '"']) && spec.ends_with(&spec[..1]),
            "Expected a string literal"
        );
        let name = &spec[1..spec.len() - 1];
        ensure!(
            !name.contains('\\'),
            "Escaped CSS imports are not supported"
        );
        if name == "tailwindcss"
            || name.strip_prefix("tailwindcss/").is_some_and(|part| {
                matches!(
                    part.strip_suffix(".css").unwrap_or(part),
                    "index" | "theme" | "preflight" | "utilities"
                )
            })
        {
            let _ = write!(result, "@import \"{name}\";");
            rest = &rest[end + 1..];
            continue;
        }
        let directory = path.parent().unwrap();
        let mut imported = match remote(directory, name, shared.enabled("httpImport"))? {
            Some((target, text)) => css(&target, Some(text), stack, shared)?,
            None => {
                let target = if name.starts_with(['.', '/']) || Path::new(name).is_absolute() {
                    shared.local(directory, name)?
                } else {
                    css_package(directory, name)?
                        .canonicalize()
                        .with_context(|| format!("Cannot resolve CSS import {name}"))?
                };
                css(&target, None, stack, shared)?
            }
        };
        let mut options = options;
        let starts = |text: &str, word: &str| {
            text.get(..word.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(word))
        };
        let group = |text: &str| -> Option<usize> {
            let mut depth = 0;
            for (index, c) in text.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 1 => return Some(index),
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            None
        };
        if starts(options, "layer(") {
            let close = group(options).context("Expected a closing parenthesis")?;
            imported = format!("@layer {}{{{imported}}}", options[6..close].trim());
            options = options[close + 1..].trim();
        } else if starts(options, "layer")
            && !options[5..].starts_with(|c: char| c.is_alphanumeric() || c == '-')
        {
            imported = format!("@layer{{{imported}}}");
            options = options[5..].trim();
        }
        if starts(options, "supports(") {
            let close = group(options).context("Expected a closing parenthesis")?;
            imported = format!("@supports ({}){{{imported}}}", options[9..close].trim());
            options = options[close + 1..].trim();
        }
        if !options.is_empty() {
            imported = format!("@media {options}{{{imported}}}");
        }
        result.push_str(&imported);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    ensure!(
        !result.to_ascii_lowercase().contains("</style"),
        "CSS cannot contain </style>: {}",
        path.display()
    );
    stack.pop();
    Ok(result)
}
