use std::rc::Rc;

// A `font` value as a canvas keeps it, like Chrome: the style, weight and small caps that are not normal, the size
// in px and the families. `em` and `%` resolve against 10px, the default canvas font. `None` keeps the old font.
pub fn font(value: &str) -> Option<String> {
  let mut longhands = Vec::new();
  crate::css::value::shorthand::expand("font", value.trim(), &mut longhands);
  let get = |name: &str| {
    longhands
      .iter()
      .find(|(key, _)| key == name)
      .map(|(_, value)| value.as_str())
  };
  let size = match get("font-size")? {
    "xx-small" => 9.,
    "x-small" => 10.,
    "small" => 13.,
    "medium" => 16.,
    "large" => 18.,
    "x-large" => 24.,
    "xx-large" => 32.,
    "xxx-large" => 48.,
    "larger" => 12.,
    "smaller" => 10. / 1.2,
    size => crate::css::value::units::Lengths::new(10., 0., 0.)
      .resolve(size, 10.)
      .filter(|size| *size >= 0.)?,
  };
  let mut out = String::new();
  if get("font-style")? == "italic" {
    out.push_str("italic ");
  }
  match get("font-weight")? {
    "normal" | "400" => {}
    "bold" | "bolder" | "700" => out.push_str("bold "),
    "lighter" => out.push_str("100 "),
    weight => {
      let weight = weight
        .parse::<f32>()
        .ok()
        .filter(|w| (1. ..=1000.).contains(w))?;
      crate::css::value::tokens::write_number(f64::from(weight), &mut out);
      out.push(' ');
    }
  }
  if get("font-variant-caps")? == "small-caps" {
    out.push_str("small-caps ");
  }
  crate::css::value::tokens::write_number(f64::from(size), &mut out);
  out.push_str("px");
  for (index, family) in crate::css::value::tokens::components(get("font-family")?, ',')
    .into_iter()
    .enumerate()
  {
    let family = family.trim().trim_matches(['"', '\'']);
    if family.is_empty() || matches!(family, "inherit" | "initial" | "unset" | "default") {
      return None;
    }
    out.push_str(if index == 0 { " " } else { ", " });
    if family.contains(' ') {
      out.push_str(&format!("\"{family}\""));
    } else {
      out.push_str(family);
    }
  }
  Some(out)
}

pub fn line(
  system: &renderer::TextSystem,
  aliases: &std::collections::HashMap<String, String>,
  font: &str,
  text: &str,
  [letter, word]: [f32; 2],
) -> (Rc<renderer::Line>, [f32; 4]) {
  let text = text.replace(['\n', '\r', '\t'], " ");
  let (size, family) = font.split_once("px").unwrap_or(("10", "sans-serif"));
  let mut face = crate::style::font(&crate::css::properties::Style::from_iter([
    (
      crate::css::properties::Prop::FONT_FAMILY,
      family.trim().into(),
    ),
    (
      crate::css::properties::Prop::FONT_WEIGHT,
      size
        .split_whitespace()
        .rev()
        .skip(1)
        .find(|part| *part == "bold" || part.parse::<u16>().is_ok())
        .unwrap_or("400")
        .into(),
    ),
    (
      crate::css::properties::Prop::FONT_STYLE,
      if size.contains("italic") {
        "italic"
      } else {
        "normal"
      }
      .into(),
    ),
  ]));
  let (first, rest) = face.family.split_once('\n').unwrap_or((&face.family, ""));
  if let Some(family) = aliases.get(first) {
    face.family = if rest.is_empty() {
      family.clone().into()
    } else {
      format!("{family}\n{rest}").into()
    };
  }
  let run = renderer::TextRun {
    len: text.len(),
    font: face,
    color: renderer::Color::BLACK,
    background_color: None,
    underline: None,
    strikethrough: None,
    overline: None,
    letter_spacing: renderer::px(letter),
    word_spacing: renderer::px(word),
    font_size: None,
    inset: Default::default(),
    nowrap: false,
    preserve: true,
    hyphenation: None,
    rtl: false,
    plaintext: false,
    embedding: None,
    clear: 0,
    tab_size: 8.,
    stroke: None,
    emphasis: None,
  };
  let font_size = size
    .split_whitespace()
    .last()
    .and_then(|v| v.parse::<f32>().ok())
    .filter(|size| size.is_finite() && *size > 0.)
    .unwrap_or(10.);
  let id = system.face(&run.font);
  let [ascent, descent, _] = system.line_metrics(id, renderer::px(font_size));
  let em = system.metrics(id).em_ascent * font_size;
  (
    system.shape_line(&text, renderer::px(font_size), &run).line,
    [f32::from(ascent), f32::from(descent), em, font_size - em],
  )
}

pub fn ink(system: &renderer::TextSystem, line: &renderer::Line) -> [f32; 4] {
  let mut ink: Option<[f32; 4]> = None;
  for glyph in &line.glyphs {
    let Some([left, right, top, bottom]) = system.glyph_bounds(glyph.font, glyph.id, glyph.size)
    else {
      continue;
    };
    let x = f32::from(glyph.x);
    let [l, r, t, b] = ink.get_or_insert([x + left, x + right, top, bottom]);
    *l = l.min(x + left);
    *r = r.max(x + right);
    *t = t.max(top);
    *b = b.min(bottom);
  }
  ink.map_or([0.; 4], |[left, right, top, bottom]| {
    [left, right, top, -bottom]
  })
}
