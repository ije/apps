use super::*;
use crate::style::tailwind::tables::VARIANTS;
use crate::style::tailwind::utility::Modifier;
use crate::style::tailwind::value::{
  arbitrary, attribute, calls, escape, integer, named_value, negate, nest, quote, split,
  statements, universal,
};

impl Config {
  pub(super) fn variant(&self, name: &str, depth: u8) -> Option<(Rank, Variant)> {
    if depth > 8 {
      return None;
    }
    let order = |name: &str| VARIANTS.iter().position(|(other, _)| *other == name);
    let (whole, (name, modifier)) = match split(name, '/')[..] {
      [only] => (name, (only, None)),
      [name, modifier] => (name, (name, Some(modifier))),
      _ => return None,
    };
    if let Some(raw) = whole.strip_prefix('[') {
      if modifier.is_some() || raw.starts_with('@') && raw.contains('&') {
        return None;
      }
      let mut value = arbitrary(name)?;
      if value.contains(['{', '}']) {
        return None;
      }
      let relative = value.starts_with(['>', '+', '~']);
      let source = if relative {
        format!(" {} {}", &value[..1], value[1..].trim_start())
      } else if let Some(at) = value.strip_prefix('@') {
        let end = at
          .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
          .unwrap_or(at.len());
        format!("@{} {}", &at[..end], at[end..].trim_start())
      } else if value.contains('&') {
        value.clone()
      } else {
        value = format!("&:is({value})");
        value.clone()
      };
      let source = match source.starts_with('@') && calls(&source) {
        true => self.resolved(&source)?,
        false => source,
      };
      let mut variant = plain(&source);
      variant.relative = relative;
      return Some(((u16::MAX, value), variant));
    }
    if let Some(index) = self.variants.iter().position(|(other, _)| other == name) {
      let rank = order(name).unwrap_or(VARIANTS.len() + index) as u16;
      // A variant that uses itself makes nothing.
      if modifier.is_some() || self.open.borrow().contains(&index) {
        return None;
      }
      self.open.borrow_mut().push(index);
      let variant = self.template(&self.variants[index].1, depth);
      self.open.borrow_mut().pop();
      return Some(((rank, String::new()), variant));
    }
    if let Some(index) = order(name)
      && !VARIANTS[index].1.is_empty()
    {
      if modifier.is_some() {
        return None;
      }
      let mut variant = plain(VARIANTS[index].1);
      if name == "hover" {
        variant.branches[0].0.push("@media (hover: hover)".into());
      }
      variant.content = matches!(name, "before" | "after");
      variant.never |= matches!(name, "*" | "**");
      return Some(((index as u16, String::new()), variant));
    }
    if modifier.is_none()
      && let Some(width) = self.raw(&format!("--breakpoint-{name}"))
    {
      return self.media(">=", width, "min");
    }
    let (root, value) = if let Some(value) = name.strip_prefix("@min-") {
      ("@min", value)
    } else if let Some(value) = name.strip_prefix("@max-") {
      ("@max", value)
    } else if let Some(value) = name.strip_prefix('@') {
      ("@min", value)
    } else {
      VARIANTS
        .iter()
        .filter(|(_, template)| template.is_empty())
        .filter_map(|(root, _)| Some((*root, name.strip_prefix(root)?.strip_prefix('-')?)))
        .max_by_key(|(root, _)| root.len())?
    };
    if value.is_empty() {
      return None;
    }
    let rank = order(root)? as u16;
    let modifier = match modifier {
      Some(text) if !matches!(root, "not" | "has" | "in") => Some(Modifier::parse(text)?),
      Some(text) => Some(Modifier::Named(text)),
      None => None,
    };
    match root {
      "group" | "peer" | "not" | "has" | "in" => {
        let named = matches!(root, "group" | "peer");
        let inner = match &modifier {
          Some(Modifier::Named(modifier)) if !named => format!("{value}/{modifier}"),
          _ => value.to_owned(),
        };
        let (inner_rank, inner) = self.variant(&inner, depth + 1)?;
        if inner.never || inner.relative && !matches!(root, "has" | "in") {
          return None;
        }
        let branches = if root == "not" {
          let [(at, selector)] = <[_; 1]>::try_from(inner.branches).ok()?;
          if at.len() > 1 {
            return None;
          }
          let mut branches = Vec::new();
          if let Some(selector) = selector {
            let list: Vec<_> = split(&selector, ',')
              .into_iter()
              .map(|part| universal(part.trim(), true))
              .collect();
            branches.push((Vec::new(), Some(format!("&:not({})", list.join(", ")))));
          }
          if let Some(rule) = at.first() {
            branches.push((vec![negate(rule)?], None));
          }
          branches
        } else {
          let prefix = match self.prefix.as_str() {
            "" => String::new(),
            prefix => format!("{prefix}\\:"),
          };
          let marker = match &modifier {
            Some(Modifier::Named(name)) if named => {
              format!(
                ":where(.{prefix}{root}\\/{})",
                &escape(&format!("/{name}"))[3..]
              )
            }
            Some(Modifier::Raw(name)) => format!(":where(.{prefix}{root}\\/{name})"),
            _ => format!(":where(.{prefix}{root})"),
          };
          let mut branches = Vec::new();
          for (at, selector) in inner.branches {
            let selector = selector?;
            let selector = match root {
              "has" => format!("&:has({})", universal(&selector, true)),
              "in" => format!(":where({}) &", universal(&selector, true)),
              _ => {
                let list = selector.replace('&', &marker);
                let list = if split(&list, ',').len() > 1 {
                  format!(":is({list})")
                } else {
                  list
                };
                let target = if root == "group" { "*" } else { "~ *" };
                format!("&:is({list} {target})")
              }
            };
            branches.push((at, Some(selector)));
          }
          branches
        };
        if branches.is_empty() {
          return None;
        }
        Some((
          (
            rank,
            format!(
              "{:05}{}{}",
              inner_rank.0,
              inner_rank.1,
              match &modifier {
                Some(Modifier::Named(name)) if named => format!("\0{name}"),
                Some(Modifier::Raw(name)) => format!("\0{name}"),
                _ => String::new(),
              }
            ),
          ),
          Variant {
            branches,
            ..Variant::default()
          },
        ))
      }
      _ if modifier.is_some() && !root.starts_with('@') => None,
      "aria" | "data" => {
        let (kind, value) = match value.starts_with(['[', '(']) {
          true => ('1', arbitrary(value)?),
          false if named_value(value) => ('0', value.to_owned()),
          false => return None,
        };
        let test = match (root, kind) {
          ("aria", '0') => format!("[aria-{value}=\"true\"]"),
          _ => format!("[{root}-{}]", quote(&value)),
        };
        attribute(&test).then(|| ((rank, format!("{kind}{value}")), plain(&format!("&{test}"))))
      }
      "nth" | "nth-last" | "nth-of-type" | "nth-last-of-type" => {
        let (kind, argument) = match value.starts_with(['[', '(']) {
          true => ('1', arbitrary(value)?),
          false if integer(value) => ('0', value.to_owned()),
          false => return None,
        };
        let pseudo = match root {
          "nth" => "nth-child",
          "nth-last" => "nth-last-child",
          _ => root,
        };
        Some((
          (rank, format!("{kind}{argument}")),
          plain(&format!("&:{pseudo}({argument})")),
        ))
      }
      "supports" => {
        if value == "bg-glass" {
          return Some(((rank, "0bg-glass".into()), plain("@media (-macos-glass)")));
        }
        let (kind, raw) = if value.starts_with(['[', '(']) {
          ('1', arbitrary(value)?)
        } else if named_value(value) {
          ('0', value.to_owned())
        } else {
          return None;
        };
        let name_end = raw
          .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
          .unwrap_or(raw.len());
        let condition = if raw[name_end..].trim_start().starts_with('(') {
          // Words between conditions need spaces, as in `(a:b)and(c:d)`.
          let mut condition = String::with_capacity(raw.len() + 8);
          let mut depth = 0;
          for part in raw.split_inclusive(['(', ')']) {
            let word = part.trim_end_matches('(').trim();
            if part.ends_with('(') && matches!(word, "and" | "or" | "not") {
              let space = if condition.is_empty() { "" } else { " " };
              condition.push_str(&format!("{space}{word} ("));
            } else {
              condition.push_str(part);
            }
            depth += i32::from(part.ends_with('(')) - i32::from(part.ends_with(')'));
          }
          if depth == 0 { condition } else { raw.clone() }
        } else {
          let raw = match raw.contains(':') {
            true => raw.clone(),
            false => format!("{raw}: var(--tw)"),
          };
          match raw.starts_with('(') && raw.ends_with(')') {
            true => raw,
            false => format!("({raw})"),
          }
        };
        let condition = self.resolved(&condition)?;
        Some((
          (rank, format!("{kind}{raw}")),
          Variant {
            branches: vec![(vec![format!("@supports {condition}")], None)],
            ..Variant::default()
          },
        ))
      }
      "min" | "max" => {
        let width = self.width("--breakpoint", value)?;
        self.media(if root == "min" { ">=" } else { "<" }, &width, root)
      }
      _ => {
        let width = self.width("--container", value)?;
        let name = match &modifier {
          Some(Modifier::Named(name)) => format!("{name} "),
          Some(Modifier::Raw(name)) => format!("{name} "),
          None => String::new(),
        };
        let operator = if root == "@max" { "<" } else { ">=" };
        let (rank, _) = self.media(operator, &width, root)?;
        Some((
          rank,
          Variant {
            branches: vec![(
              vec![format!("@container {name}(width {operator} {width})")],
              None,
            )],
            ..Variant::default()
          },
        ))
      }
    }
  }

  fn width(&self, namespace: &str, value: &str) -> Option<String> {
    let width = match value.starts_with(['[', '(']) {
      true => arbitrary(value)?,
      false => self.raw(&self.key(namespace, value)?)?.to_owned(),
    };
    if width.contains("var(") {
      return None;
    }
    self.resolved(&width)
  }

  pub(super) fn media(&self, operator: &str, width: &str, root: &str) -> Option<(Rank, Variant)> {
    // Tailwind groups breakpoints by unit (or function name) and only then by number, so `px` values sort
    // before `rem` values.
    let bucket = match width.find('(') {
      Some(end) => width[..end].to_owned(),
      None => width.replace(|c: char| c.is_ascii_digit() || c == '.', ""),
    };
    let sign = usize::from(width.starts_with(['+', '-']));
    let end = width[sign..]
      .find(|c: char| !(c.is_ascii_digit() || c == '.'))
      .map_or(width.len(), |end| sign + end);
    let key = match width[..end].parse::<f64>() {
      Ok(value) => {
        let value = if operator == "<" { -value } else { value } + 0.;
        let bits = value.to_bits();
        let bits = if value < 0. { !bits } else { bits | 1 << 63 };
        format!("{bucket}\0{bits:016x}")
      }
      Err(_) => format!("{bucket}\0\u{1}{width}"),
    };
    Some((
      (
        VARIANTS.iter().position(|(name, _)| *name == root)? as u16,
        key,
      ),
      Variant {
        branches: vec![(vec![format!("@media (width {operator} {width})")], None)],
        ..Variant::default()
      },
    ))
  }
}

fn plain(source: &str) -> Variant {
  let branch = match source.starts_with('@') {
    true => (vec![source.to_owned()], None),
    false => (Vec::new(), Some(source.to_owned())),
  };
  Variant {
    branches: vec![branch],
    ..Variant::default()
  }
  .closed()
}

impl Variant {
  // Other variants can't wrap a pseudo-element or at-rules that are no conditions.
  fn closed(mut self) -> Self {
    self.never = self.branches.iter().any(|(at, selector)| {
      selector
        .as_ref()
        .is_some_and(|selector| selector.contains("::"))
        || at.iter().any(|rule| {
          !["@media", "@supports", "@container"]
            .iter()
            .any(|name| rule.starts_with(name))
        })
    });
    self
  }
}

impl Config {
  // The variant of `@custom-variant`, whose source is a list of selectors and at-rules, or a block.
  fn template(&self, source: &str, depth: u8) -> Variant {
    let mut variant = Variant::default();
    if source.contains("@slot") {
      self.slots(source, &[], None, &mut variant.branches, depth);
    } else {
      let (rules, selectors): (Vec<_>, Vec<_>) = split(source, ',')
        .into_iter()
        .map(str::trim)
        .partition(|part| part.starts_with('@'));
      if !selectors.is_empty() {
        variant
          .branches
          .push((Vec::new(), Some(selectors.join(","))));
      }
      for rule in rules {
        variant.branches.push((vec![self.functions(rule)], None));
      }
    }
    variant.closed()
  }

  fn slots(
    &self,
    source: &str,
    at: &[String],
    selector: Option<&str>,
    out: &mut Vec<(Vec<String>, Option<String>)>,
    depth: u8,
  ) {
    if depth > 8 || out.len() > 8 {
      return;
    }
    for (prelude, block) in statements(source) {
      if self.spend(prelude.len()).is_none() {
        return;
      }
      match block {
        None if prelude == "@slot" => out.push((at.to_vec(), selector.map(str::to_owned))),
        None => {}
        Some(inner) if prelude.starts_with("@variant ") => {
          let base = selector.unwrap_or("&");
          for (rules, selectors) in self
            .variant_list(&prelude[9..], base, depth + 1)
            .into_iter()
            .flatten()
          {
            let mut at = at.to_vec();
            at.extend(rules);
            let list = selectors.join(",");
            let selector = (list != "&").then_some(list.as_str());
            self.slots(inner, &at, selector, out, depth + 1);
          }
        }
        Some(inner) if prelude.starts_with('@') => {
          let mut at = at.to_vec();
          at.push(self.functions(prelude));
          self.slots(inner, &at, selector, out, depth + 1);
        }
        Some(inner) => {
          let nested = match selector {
            Some(outer) if prelude.contains('&') => nest(prelude, outer),
            Some(outer) => format!("{outer} {prelude}"),
            None if prelude.contains('&') => prelude.to_owned(),
            None => format!("& {prelude}"),
          };
          self.slots(inner, at, Some(&nested), out, depth + 1);
        }
      }
    }
  }
}
