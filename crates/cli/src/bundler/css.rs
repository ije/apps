use super::{
  Shared,
  resolve::{exported, remote},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
  fmt::Write,
  fs,
  path::{Path, PathBuf},
};

fn css_package(directory: &Path, name: &str) -> Result<PathBuf> {
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
      shared.files.borrow_mut().insert(path.to_owned());
      ensure!(
        path
          .extension()
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
      let space = if options.is_empty() { "" } else { " " };
      let _ = write!(result, "@import \"{name}\"{space}{options};");
      rest = &rest[end + 1..];
      continue;
    }
    let directory = path.parent().unwrap();
    let mut imported = match remote(
      directory,
      name,
      shared.enabled(apps_runtime::bundle::Feature::HttpImport),
    )? {
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
    let starts = |text: &str, word: &str| {
      text
        .get(..word.len())
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
    // Tailwind's options apply to the theme and the Tailwind imports of the file.
    let (mut theme, mut tailwind, mut media) = (String::new(), String::new(), String::new());
    let (mut words, mut reference) = (Vec::new(), false);
    let (mut start, mut depth) = (0, 0);
    for (index, c) in options.char_indices() {
      depth += i32::from(c == '(') - i32::from(c == ')');
      if c.is_whitespace() && depth == 0 {
        words.push(&options[start..index]);
        start = index + 1;
      }
    }
    words.push(&options[start..]);
    for word in words.into_iter().filter(|word| !word.is_empty()) {
      let target = match word.strip_prefix("theme(") {
        Some(inner) => {
          let inner = inner.strip_suffix(')').unwrap_or(inner);
          reference |= inner
            .split_ascii_whitespace()
            .any(|word| word == "reference");
          let _ = write!(theme, " {inner} ");
          &mut tailwind
        }
        None if word.starts_with("prefix(") => {
          let _ = write!(theme, " {word} ");
          &mut tailwind
        }
        None if word == "important" => &mut tailwind,
        None if word == "reference" => {
          reference = true;
          continue;
        }
        None if word.starts_with("source(") => continue,
        None => &mut media,
      };
      let _ = write!(target, " {word}");
    }
    let mut options = media.trim();
    let mut layer = None;
    if starts(options, "layer(") {
      let close = group(options).context("Expected a closing parenthesis")?;
      layer = Some(options[6..close].trim().to_owned());
    }
    if !theme.is_empty() || !tailwind.is_empty() || layer.is_some() {
      imported = annotate(&imported, &theme, &tailwind, layer.as_deref());
    }
    if reference {
      // A referenced file gives its theme, utilities and variants, and no rules.
      result.push_str(&configuration(&imported, &theme));
      rest = &rest[end + 1..];
      continue;
    }
    if let Some(layer) = &layer {
      let close = group(options).context("Expected a closing parenthesis")?;
      // Tailwind's imports stay outside of the layer, with its name.
      let (imports, rest) = imported.split_at(match imported.rfind("@import \"tailwindcss") {
        Some(last) => last + imported[last..].find(';').map_or(0, |end| end + 1),
        None => 0,
      });
      imported = format!("{imports}@layer {layer}{{{rest}}}");
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

// The rules of a stylesheet that configure Tailwind, with the theme as a reference.
fn configuration(source: &str, theme: &str) -> String {
  let bytes = source.as_bytes();
  let (mut result, mut start, mut pos, mut quote, mut depth, mut open) =
    (String::new(), 0, 0, 0, 0, 0);
  while pos < bytes.len() {
    let byte = bytes[pos];
    pos += 1;
    if quote != 0 {
      pos += usize::from(byte == b'\\');
      quote = if byte == quote { 0 } else { quote };
      continue;
    }
    if bytes[pos - 1..].starts_with(b"/*") {
      let end = source[pos..].find("*/");
      pos = end.map_or(bytes.len(), |end| pos + end + 2);
      if depth == 0 && source[start..pos].trim_start().starts_with("/*") {
        start = pos;
      }
      continue;
    }
    match byte {
      b'"' | b'\'' => quote = byte,
      b'{' => {
        if depth == 0 {
          open = pos;
        }
        depth += 1;
      }
      b'}' => depth -= 1,
      _ => {}
    }
    if depth > 0 || !matches!(byte, b';' | b'}') {
      continue;
    }
    let item = source[start..pos].trim();
    let name = item.split(|c: char| c.is_whitespace() || "({;".contains(c));
    match name.into_iter().next().unwrap_or_default() {
      "@theme" if byte == b'}' => {
        let _ = write!(result, "@theme{theme} reference{}", &item[6..]);
      }
      "@utility" | "@custom-variant" => result.push_str(item),
      "@variant" if byte == b';' || item.contains("@slot") => result.push_str(item),
      "@import" if item.contains("\"tailwindcss") => {
        result.push_str("@reference \"tailwindcss\";");
        if let Some(prefix) = item.find("prefix(").map(|at| &item[at..]) {
          let end = prefix.find(')').map_or(prefix.len(), |end| end + 1);
          let _ = write!(result, "@theme {} reference{{}}", &prefix[..end]);
        }
      }
      "@layer" if byte == b'}' => {
        result.push_str(&configuration(&source[open..pos - 1], theme));
      }
      _ => {}
    }
    start = pos;
  }
  result
}

// Adds the options of an import to the `@theme` rules and the Tailwind imports of its file.
fn annotate(source: &str, theme: &str, tailwind: &str, layer: Option<&str>) -> String {
  let bytes = source.as_bytes();
  let (mut result, mut copied, mut pos, mut quote) = (String::new(), 0, 0, 0);
  while pos < bytes.len() {
    let byte = bytes[pos];
    if quote != 0 {
      pos += usize::from(byte == b'\\');
      quote = if byte == quote { 0 } else { quote };
    } else if matches!(byte, b'"' | b'\'') {
      quote = byte;
    } else if bytes[pos..].starts_with(b"/*") {
      pos = source[pos..]
        .find("*/")
        .map_or(bytes.len(), |end| pos + end + 2);
      continue;
    } else if bytes[pos..].starts_with(b"@theme") {
      result.push_str(&source[copied..pos + 6]);
      result.push_str(theme);
      copied = pos + 6;
    } else if bytes[pos..].starts_with(b"@import \"tailwindcss")
      && let Some(end) = source[pos..].find(';')
    {
      result.push_str(&source[copied..pos + end]);
      result.push_str(tailwind);
      if let Some(layer) = layer {
        let _ = write!(result, " layer({layer})");
      }
      copied = pos + end;
      pos = copied;
    }
    pos += 1;
  }
  result.push_str(&source[copied.min(source.len())..]);
  result
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
  // The parentheses around a position, and whether each is of an `image-set()`, whose strings are URLs.
  let mut groups = Vec::new();
  while pos < bytes.len() {
    if bytes[pos..].starts_with(b"/*") {
      pos = source[pos + 2..]
        .find("*/")
        .map_or(bytes.len(), |end| pos + end + 4);
    } else if matches!(bytes[pos], b'"' | b'\'') {
      let end = string(pos);
      if groups.last() == Some(&true)
        && let Some(url) = shared.asset(directory, &source[pos..end])
      {
        result.push_str(&source[copied..pos]);
        result.push_str("url(\"");
        result.push_str(&url);
        result.push_str("\")");
        copied = end;
      }
      pos = end;
    } else if matches!(bytes[pos], b'(' | b')') {
      match bytes[pos] {
        b'(' => groups.push(pos >= 9 && bytes[pos - 9..pos].eq_ignore_ascii_case(b"image-set")),
        _ => _ = groups.pop(),
      }
      pos += 1;
    } else if bytes[pos..].len() >= 4 && bytes[pos..pos + 4].eq_ignore_ascii_case(b"url(") {
      groups.push(false);
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
