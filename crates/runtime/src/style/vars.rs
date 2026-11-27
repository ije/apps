use crate::css::{
  properties::Prop, properties::Registry, properties::Style, properties::Vars,
  properties::physical, value::color::serialize as serialize_color,
  value::color::system as system_color, value::grammar::computed, value::grammar::fold,
  value::grammar::registered_matches, value::shorthand::expand,
};
use crate::style::used::dark;
use std::rc::Rc;
pub(crate) fn var(vars: &Vars, prop: Prop) -> Option<&Rc<str>> {
  vars
    .binary_search_by_key(&prop, |entry| entry.0)
    .ok()
    .map(|index| &vars[index].1)
}

// The next `var(` outside quoted strings.
pub(crate) fn next_var(value: &str) -> Option<usize> {
  let bytes = value.as_bytes();
  let mut index = 0;
  loop {
    index += memchr::memchr3(b'v', b'"', b'\'', bytes.get(index..)?)?;
    match bytes[index] {
      b'v' if bytes[index..].starts_with(b"var(") => return Some(index),
      b'v' => index += 1,
      quote => {
        index += 1;
        while let Some(&byte) = bytes.get(index)
          && byte != quote
        {
          index += if byte == b'\\' { 2 } else { 1 };
        }
        index += 1;
      }
    }
  }
}

pub(crate) fn references(value: &str, names: &mut Option<Vec<Prop>>, missing: &mut Vec<Box<str>>) {
  let mut rest = value;
  while let Some(list) = names.as_mut()
    && let Some(start) = next_var(rest)
  {
    rest = &rest[start + 4..];
    let name = rest[..rest.find([',', ')']).unwrap_or(rest.len())].trim();
    match Prop::find(name) {
      Some(prop) => list.push(prop),
      None if !missing.iter().any(|old| **old == *name) => missing.push(name.into()),
      None => {}
    }
  }
}

// `var()` results stop at this many bytes, like in browsers, so references that fan out cannot grow exponentially.
pub(crate) const SUBSTITUTION_LIMIT: usize = 1 << 20;

pub(crate) fn substitute(
  value: &str,
  lookup: &mut dyn FnMut(Prop) -> Option<Rc<str>>,
  out: &mut String,
  nesting: u8,
) -> bool {
  let word = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '%' | '#');
  let mut rest = value;
  while let Some(start) = next_var(rest) {
    if rest[..start].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '-' | '_')) {
      out.push_str(&rest[..start + 4]);
      rest = &rest[start + 4..];
      continue;
    }
    out.push_str(&rest[..start]);
    let body = &rest[start + 4..];
    let (mut depth, mut comma, mut end, mut quote, mut escaped) = (1, None, None, None, false);
    for (index, c) in body.char_indices() {
      if escaped {
        escaped = false;
        continue;
      }
      match c {
        '\\' => escaped = true,
        _ if quote.is_some() => {
          if quote == Some(c) {
            quote = None;
          }
        }
        '"' | '\'' => quote = Some(c),
        '(' => depth += 1,
        ')' => {
          depth -= 1;
          if depth == 0 {
            end = Some(index);
            break;
          }
        }
        ',' if depth == 1 && comma.is_none() => comma = Some(index),
        _ => {}
      }
    }
    let Some(end) = end else {
      return false;
    };
    let mark = out.len();
    let name = body[..comma.unwrap_or(end)].trim();
    match name
      .starts_with("--")
      .then(|| Prop::find(name))
      .flatten()
      .and_then(&mut *lookup)
    {
      Some(resolved) => out.push_str(&resolved),
      None => match comma {
        // Nested fallbacks recurse, and so do the references in them, so their depth is capped.
        Some(comma)
          if nesting < 32 && substitute(body[comma + 1..end].trim(), lookup, out, nesting + 1) => {}
        _ => return false,
      },
    }
    rest = &body[end + 1..];
    // `var()` substitutes tokens, so `var(--a)var(--b)` stays two values.
    if out[mark..].starts_with(word) && out[..mark].ends_with(|c: char| word(c) || c == ')') {
      out.insert(mark, ' ');
    }
    if out.ends_with(|c: char| word(c) || c == ')') && rest.starts_with(word) {
      out.push(' ');
    }
    if out.len() > SUBSTITUTION_LIMIT {
      return false;
    }
  }
  out.push_str(rest);
  true
}

pub(crate) struct Substitution<'a> {
  pub(crate) declared: &'a [(Prop, Rc<str>)],
  pub(crate) state: Vec<(u8, usize, Option<Rc<str>>)>,
  pub(crate) inherited: &'a Vars,
  pub(crate) parent: Option<&'a Style>,
  pub(crate) vars: Option<&'a Vars>,
  pub(crate) registry: &'a Registry,
  pub(crate) depth: usize,
  pub(crate) cycle: usize,
}

impl Substitution<'_> {
  pub(crate) fn get(&mut self, prop: Prop) -> Option<Rc<str>> {
    let Ok(index) = self.declared.binary_search_by_key(&prop, |entry| entry.0) else {
      return var(self.inherited, prop)
        .cloned()
        .or_else(|| self.registry.get(&prop)?.2.clone());
    };
    match self.state[index].0 {
      2 => return self.state[index].2.clone(),
      1 => {
        self.cycle = self.cycle.min(self.state[index].1);
        return None;
      }
      _ => {}
    }
    let raw = self.declared[index].1.clone();
    let registered = self.registry.get(&prop);
    let (parent, vars) = (self.parent, self.vars);
    let initial = || registered.and_then(|(_, _, initial)| initial.clone());
    let inherit = || {
      match vars {
        Some(vars) => var(vars, prop).cloned(),
        None => parent.and_then(|parent| parent.value(prop).cloned()),
      }
      .or_else(initial)
    };
    let unset = || {
      if registered.is_some_and(|(_, inherits, _)| !inherits) {
        initial()
      } else {
        inherit()
      }
    };
    let value = match &*raw {
      "inherit" => inherit(),
      "initial" => initial(),
      "unset" | "revert" | "revert-layer" => unset(),
      _ => {
        let mut value = Some(raw.clone());
        if raw.contains("var(") {
          // Each nested reference recurses, so chains are invalid past this depth.
          if self.depth > 128 {
            value = None;
          } else {
            let (depth, outer) = (self.depth, self.cycle);
            self.state[index] = (1, depth, None);
            (self.depth, self.cycle) = (depth + 1, usize::MAX);
            let mut out = String::new();
            let ok = substitute(&raw, &mut |prop| self.get(prop), &mut out, 0);
            let reached = self.cycle;
            self.depth = depth;
            self.cycle = outer.min(if reached < depth { reached } else { usize::MAX });
            // A cycle is invalid at computed-value time, so a registered property in it is unset.
            value = (ok && reached > depth).then(|| out.into());
          }
        }
        match (value, registered) {
          (Some(value), Some((syntax, ..))) if syntax.trim() != "*" => {
            match registered_matches(syntax, &value) {
              true => Some(computed(syntax, &value, None).0.into()),
              false => unset(),
            }
          }
          (Some(value), Some(_)) if value.trim().is_empty() => unset(),
          (None, Some(_)) => unset(),
          (value, _) => value,
        }
      }
    };
    self.state[index] = (2, 0, value.clone());
    value
  }
}

pub(crate) fn resolve_style(
  style: &mut Style,
  parent: Option<&Style>,
  parent_vars: Option<&Vars>,
  declared: &[(Prop, Rc<str>)],
  registry: &Registry,
  theme: u32,
) {
  if !declared.is_empty() {
    // Only custom properties are declared or referenced here.
    let inherited = style.vars().clone();
    let mut resolver = Substitution {
      declared,
      state: vec![(0, 0, None); declared.len()],
      inherited: &inherited,
      parent,
      vars: parent_vars,
      registry,
      depth: 0,
      cycle: usize::MAX,
    };
    let values: Vec<_> = declared
      .iter()
      .map(|&(prop, _)| (prop, resolver.get(prop)))
      .collect();
    if values
      .iter()
      .any(|(prop, value)| var(&inherited, *prop) != value.as_ref())
    {
      let mut vars = (**style.vars()).clone();
      for (prop, value) in values {
        match (vars.binary_search_by_key(&prop, |entry| entry.0), value) {
          (Ok(index), Some(value)) => vars[index].1 = value,
          (Err(index), Some(value)) => vars.insert(index, (prop, value)),
          (Ok(index), None) => {
            vars.remove(index);
          }
          (Err(_), None) => {}
        }
      }
      style.set_vars(Rc::new(vars));
    }
  }
  // The longhands of a `var()` shorthand share one pending value, so it is substituted and expanded once.
  let mut values: Vec<(Rc<str>, Option<String>, Option<Vec<_>>)> = Vec::new();
  let resolved: Vec<_> = style
    .own()
    .filter(|(_, value)| next_var(value).is_some())
    .map(|(prop, value)| {
      let shared = value
        .starts_with('\u{1}')
        .then(|| values.iter().position(|(raw, ..)| raw == value))
        .flatten();
      let index = shared.unwrap_or_else(|| {
        let mut out = String::new();
        let ok = substitute(
          value,
          &mut |name| {
            style
              .value(name)
              .cloned()
              .or_else(|| registry.get(&name)?.2.clone())
          },
          &mut out,
          0,
        );
        values.push((value.clone(), ok.then_some(out), None));
        values.len() - 1
      });
      (prop, index)
    })
    .collect();
  for (prop, index) in resolved {
    let (raw, substituted, expanded) = &mut values[index];
    let value = match substituted
      .as_deref()
      .and_then(|value| value.strip_prefix('\u{1}')?.split_once('\u{1}'))
    {
      Some((shorthand, value)) => expanded
        .get_or_insert_with(|| {
          let mut expanded = Vec::new();
          if !crate::css::sheet::parser::invalid(shorthand, value) {
            expand(shorthand, value.trim(), &mut expanded);
          }
          expanded
        })
        .iter()
        .find(|(name, _)| {
          name == prop.name() || Prop::find(name).is_some_and(|name| physical(name, style) == prop)
        })
        .map(|(_, value)| value.clone()),
      // Values that start with U+0001 can be shared by other properties.
      None if raw.starts_with('\u{1}') => substituted.clone(),
      None => substituted.take(),
    }
    .filter(|value| !crate::css::sheet::parser::invalid(prop.name(), value))
    .map(|value| fold(prop.name(), &value).unwrap_or(value));
    match value.map(Rc::from).or_else(|| {
      parent
        .filter(|_| prop.inherited())
        .and_then(|parent| parent.value(prop).cloned())
    }) {
      Some(value) => style.insert(prop, value),
      None => {
        style.remove(prop);
      }
    }
  }
  let dark = dark(style, theme);
  style.retain(|prop, slot| {
    if prop.is_custom() || !slot.contains("light-dark(") {
      return true;
    }
    // Mark removed ranges, then copy once. Repeated in-place replacements are quadratic.
    let bytes = slot.as_bytes();
    let mut dropped = vec![0i32; bytes.len() + 1];
    let mut open: Vec<(usize, Option<usize>, u32)> = Vec::new();
    let (mut quote, mut index) = (0u8, 0);
    while index < bytes.len() {
      match bytes[index] {
        b'\\' => index += 1,
        byte if quote != 0 => quote = if byte == quote { 0 } else { quote },
        byte @ (b'"' | b'\'') => quote = byte,
        b'(' => open.push((index, None, 0)),
        b',' => {
          if let Some((_, comma, commas)) = open.last_mut() {
            comma.get_or_insert(index);
            *commas += 1;
          }
        }
        b')' => {
          if let Some((start, Some(comma), 1)) = open.pop()
            && slot[..start].ends_with("light-dark")
          {
            let parts = [start + 1..comma, comma + 1..index];
            if parts
              .iter()
              .all(|part| !slot[part.clone()].trim().is_empty())
            {
              let kept = parts[usize::from(dark)].clone();
              let text = &slot[kept.clone()];
              dropped[start - 10] += 1;
              dropped[kept.start + text.len() - text.trim_start().len()] -= 1;
              dropped[kept.start + text.trim_end().len()] += 1;
              dropped[index + 1] -= 1;
            }
          }
        }
        _ => {}
      }
      index += 1;
    }
    let mut value = String::with_capacity(slot.len());
    let (mut depth, mut from) = (0, 0);
    for (index, &change) in dropped
      .iter()
      .enumerate()
      .filter(|(_, change)| **change != 0)
    {
      if depth == 0 {
        value.push_str(&slot[from..index]);
      }
      depth += change;
      if depth == 0 {
        from = index;
      }
    }
    value.push_str(&slot[from..]);
    *slot = value.into();
    true
  });
  style.retain(|prop, value| {
    if (prop.name().ends_with("color")
      || prop.name().starts_with("-terminal-")
      || matches!(prop, Prop::FILL | Prop::STROKE))
      && value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'-')
      && let Some(system) = system_color(value, dark)
    {
      *value = system.into();
    }
    true
  });
  if !style.contains(Prop::COLOR) && parent.is_some_and(|parent| !parent.contains(Prop::COLOR)) {
    style.insert(
      Prop::COLOR,
      system_color("canvastext", dark).unwrap_or_default(),
    );
  }
  let convert = |value: &str, current: &str| {
    if !value.starts_with("color-mix(")
      && !value.contains("(from ")
      && !value.starts_with("contrast-color(")
    {
      return None;
    }
    serialize_color(&value.to_ascii_lowercase().replace("currentcolor", current))
  };
  let inherited = parent
    .and_then(|parent| parent.get(Prop::COLOR))
    .unwrap_or("canvastext");
  if let Some(value) = style
    .get(Prop::COLOR)
    .and_then(|value| convert(value, inherited))
  {
    style.insert(Prop::COLOR, value);
  }
  let current = style
    .get(Prop::COLOR)
    .filter(|value| !value.eq_ignore_ascii_case("currentcolor"))
    .unwrap_or(inherited)
    .to_owned();
  style.retain(|prop, value| {
    if prop != Prop::COLOR
      && !prop.is_custom()
      && (prop.name().ends_with("color")
        || prop.name().starts_with("-terminal-")
        || matches!(prop, Prop::FILL | Prop::STROKE))
      && let Some(converted) = convert(value, &current)
    {
      *value = converted.into();
    }
    true
  });
}
