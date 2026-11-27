use crate::css::{sheet::parser::FONT_FORMATS, sheet::parser::FONT_TECHS};
use std::collections::HashMap;

pub(crate) fn sources(src: &str) -> Vec<(&'static str, String)> {
  crate::css::value::tokens::components(src, ',')
    .into_iter()
    .filter_map(|part| {
      let (function, rest) = part.trim().split_once('(')?;
      let rest = rest.trim_start();
      let (value, hints) = match rest.as_bytes().first()? {
        &quote @ (b'"' | b'\'') => {
          let (value, tail) = rest[1..].split_once(quote as char)?;
          (value, tail.split_once(')')?.1)
        }
        _ => {
          let (value, tail) = rest.split_once(')')?;
          (value.trim(), tail)
        }
      };
      let hints = hints.to_ascii_lowercase();
      let known = |function: &str, known: &[&str], all: bool| {
        hints.split_once(function).is_none_or(|(_, tail)| {
          let mut items = tail.split(')').next().unwrap_or("").split(',').map(|item| {
            let item = item.trim().trim_matches(['"', '\'']);
            known.contains(&item.strip_suffix("-variations").unwrap_or(item))
          });
          if all {
            items.all(|known| known)
          } else {
            items.any(|known| known)
          }
        })
      };
      match function.trim().to_ascii_lowercase().as_str() {
        "local" => Some(("local", value.to_owned())),
        "url" if known("format(", &FONT_FORMATS, false) && known("tech(", &FONT_TECHS, true) => {
          Some(("url", value.to_owned()))
        }
        _ => None,
      }
    })
    .collect()
}

pub(crate) type Face = (
  String,
  String,
  Option<String>,
  &'static str,
  [Option<String>; 3],
);

pub(crate) fn ranges(text: &str) -> Vec<[u32; 2]> {
  text
    .split(',')
    .filter_map(|range| {
      let range = range.trim().strip_prefix(['U', 'u'])?.strip_prefix('+')?;
      let (low, high) = range.split_once('-').unwrap_or((range, range));
      let hex = |text: &str, any: u32| {
        text.chars().try_fold(0u32, |value, c| {
          let digit = if c == '?' { Some(any) } else { c.to_digit(16) };
          value.checked_mul(16)?.checked_add(digit?)
        })
      };
      let (low, high) = (hex(low, 0)?, hex(high, 15)?);
      (low <= high).then_some([low, high.min(0x10ffff)])
    })
    .collect()
}

/// The descriptors of an `@font-face` for face matching. Without one, the font says.
pub(crate) fn rules(
  range: Option<&str>,
  [weight, style, stretch]: [Option<&str>; 3],
) -> renderer::FaceRules {
  let pair = |value: Option<&str>, one: &dyn Fn(&str) -> Option<f32>| {
    let mut values = value.unwrap_or("").split_ascii_whitespace().map(one);
    match (values.next().flatten(), values.next().flatten()) {
      (Some(a), Some(b)) => Some([a.min(b), a.max(b)]),
      (Some(a), None) => Some([a; 2]),
      _ => None,
    }
  };
  let ranges = ranges(range.unwrap_or(""));
  renderer::FaceRules {
    weight: pair(weight, &|value| match value {
      "normal" => Some(400.),
      "bold" => Some(700.),
      value => value.parse().ok(),
    }),
    style: match style {
      Some("normal") => Some(renderer::FontStyle::Normal),
      Some(style) if style.starts_with("italic") => Some(renderer::FontStyle::Italic),
      Some(style) if style.starts_with("oblique") => Some(renderer::FontStyle::Oblique),
      _ => None,
    },
    stretch: pair(stretch, &crate::style::stretch),
    ranges: match ranges[..] {
      [[0, 0x10ffff]] => Vec::new(),
      _ => ranges,
    },
  }
}

#[derive(Default)]
pub struct Fonts {
  pub(crate) aliases: HashMap<String, String>,
  pub(crate) files: HashMap<String, Option<String>>,
  // The rules whose faces were added, at their places.
  pub(crate) placed: Vec<Option<crate::css::sheet::FontFace>>,
  pub(crate) loads: u32,
  pub(crate) text: (u64, Vec<u64>),
  pub(crate) system: Option<std::rc::Rc<renderer::TextSystem>>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sources_skip_unsupported_formats_and_techs() {
    let src = r#"local("Inter Regular"), url(a.woff2) format("woff2"), url('b.eot') format("embedded-opentype"),
            url(c.svg#c) format(svg), url(d.ttf) format("truetype") tech(color-COLRv1),
            url(e.woff2) format("woff2-variations"), URL(f.otf) tech(variations, features-opentype),
            url("g).woff") format(woff), local(Arial   Bold), url(h.otc) format(collection)"#;
    assert_eq!(
      sources(src),
      [
        ("local", "Inter Regular"),
        ("url", "a.woff2"),
        ("url", "e.woff2"),
        ("url", "f.otf"),
        ("url", "g).woff"),
        ("local", "Arial   Bold"),
        ("url", "h.otc"),
      ]
      .map(|(kind, value)| (kind, value.to_owned()))
    );
  }
}
