pub(crate) mod clip;
pub(crate) mod filter;
pub(crate) mod gradient;
pub(crate) mod offset;
pub(crate) mod transform;

use crate::css;
use crate::css::properties::Prop;
use crate::{css::properties::Style, dom::Document};
use rustc_hash::FxHashMap;

// Find IDs through the tree walk. The arena includes removed nodes and can greatly exceed the live page.
#[derive(Default)]
pub(crate) struct Ids {
  map: std::cell::OnceCell<FxHashMap<String, Vec<usize>>>,
  walk: Option<std::rc::Rc<Vec<(u32, u32)>>>,
}

impl Ids {
  pub(crate) fn of(walk: std::rc::Rc<Vec<(u32, u32)>>) -> Self {
    Self {
      map: Default::default(),
      walk: Some(walk),
    }
  }
}

pub(crate) fn index<'a>(ids: &'a Ids, doc: &Document) -> &'a FxHashMap<String, Vec<usize>> {
  ids.map.get_or_init(|| {
    let named = |node: usize| Some((node, doc.nodes.get(node)?.attrs.get("id")?));
    let mut nodes: Vec<_> = match &ids.walk {
      Some(walk) => walk
        .iter()
        .filter_map(|&(node, _)| named(node as usize))
        .collect(),
      None => (0..doc.nodes.len()).filter_map(named).collect(),
    };
    nodes.sort_unstable_by_key(|&(node, _)| node);
    let mut map = FxHashMap::<String, Vec<usize>>::default();
    for (node, id) in nodes {
      map.entry(id.clone()).or_default().push(node);
    }
    map
  })
}

pub(crate) fn opacity(style: &css::properties::Style) -> f32 {
  style
    .get(Prop::OPACITY)
    .and_then(|value| {
      value
        .strip_suffix('%')
        .map_or(value.parse::<f32>().ok(), |v| {
          v.parse::<f32>().ok().map(|v| v / 100.)
        })
    })
    .filter(|v| v.is_finite())
    .unwrap_or(1.)
    .clamp(0., 1.)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Establishes {
  pub(crate) stacking: bool,
  pub(crate) absolute: bool,
  pub(crate) fixed: bool,
  pub(crate) layer: bool,
  pub(crate) transformed: bool,
  // The used `transform-style` is `preserve-3d`.
  pub(crate) preserves: bool,
}

// Share this decision across build, cull and cache keys. Callers add flex/grid z-index and opacity layers.
pub(crate) fn establishes(style: &Style) -> Establishes {
  let set = |props: &[Prop]| {
    props
      .iter()
      .any(|&prop| style.get(prop).is_some_and(|value| value != "none"))
  };
  if style
    .get(Prop::DISPLAY)
    .is_some_and(|value| matches!(value, "contents" | "none"))
  {
    return Establishes::default();
  }
  let position = style.get(Prop::POSITION).unwrap_or("static");
  let transformed = set(&TRANSFORM_PROPS);
  let layer = set(&[
    Prop::FILTER,
    Prop::WEBKIT_FILTER,
    Prop::CLIP_PATH,
    Prop::WEBKIT_CLIP_PATH,
    Prop::MASK_IMAGE,
    Prop::WEBKIT_MASK_IMAGE,
    Prop::BACKDROP_FILTER,
    Prop::WEBKIT_BACKDROP_FILTER,
  ]) || style
    .get(Prop::MIX_BLEND_MODE)
    .is_some_and(|value| value != "normal")
    || style.get(Prop::ISOLATION) == Some("isolate")
    || cfg!(target_os = "macos")
      && matches!(style.get(Prop::MACOS_GLASS), Some("regular" | "clear"));
  let contained = style.get(Prop::CONTAIN).is_some_and(|value| {
    ["layout", "paint", "strict", "content"]
      .iter()
      .any(|kind| value.contains(kind))
  }) || style
    .get(Prop::CONTENT_VISIBILITY)
    .is_some_and(|value| value != "visible");
  let mut preserves = style.get(Prop::TRANSFORM_STYLE) == Some("preserve-3d")
    && !layer
    && opacity(style) >= 1.
    && [Prop::OVERFLOW_X, Prop::OVERFLOW_Y]
      .into_iter()
      .all(|prop| style.get(prop).is_none_or(|value| value == "visible"))
    && style.get(Prop::CONTAIN).is_none_or(|value| {
      (value.split(' ')).all(|kind| !matches!(kind, "paint" | "strict" | "content"))
    })
    && style
      .get(Prop::CONTENT_VISIBILITY)
      .is_none_or(|value| value == "visible")
    && style.get(Prop::CLIP).is_none_or(|value| value == "auto");
  let (mut wills, mut will_stack, mut will_position) = (false, false, false);
  for name in style
    .get(Prop::WILL_CHANGE)
    .into_iter()
    .flat_map(|value| value.split(','))
  {
    let is = |names: &[&str]| {
      names
        .iter()
        .any(|known| name.trim().eq_ignore_ascii_case(known))
    };
    wills |= is(&[
      "transform",
      "translate",
      "rotate",
      "scale",
      "perspective",
      "offset-path",
      "filter",
      "-webkit-filter",
      "backdrop-filter",
      "-webkit-backdrop-filter",
      "transform-style",
      "contain",
      "content-visibility",
    ]);
    will_position |= is(&["position"]);
    will_stack |= is(&[
      "opacity",
      "clip-path",
      "-webkit-clip-path",
      "mask",
      "mask-image",
      "-webkit-mask",
      "-webkit-mask-image",
      "mix-blend-mode",
      "isolation",
    ]);
    preserves &= !is(&[
      "opacity",
      "filter",
      "-webkit-filter",
      "backdrop-filter",
      "-webkit-backdrop-filter",
      "clip-path",
      "-webkit-clip-path",
      "mask",
      "mask-image",
      "-webkit-mask",
      "-webkit-mask-image",
      "mix-blend-mode",
      "isolation",
    ]);
  }
  let fixed = transformed
    || contained
    || wills
    || set(&[
      Prop::PERSPECTIVE,
      Prop::FILTER,
      Prop::WEBKIT_FILTER,
      Prop::BACKDROP_FILTER,
      Prop::WEBKIT_BACKDROP_FILTER,
    ])
    || style.get(Prop::TRANSFORM_STYLE) == Some("preserve-3d");
  let masked = set(&[Prop::MASK, Prop::WEBKIT_MASK]);
  preserves &= !masked;
  Establishes {
    stacking: fixed
      || layer
      || will_stack
      || will_position
      || matches!(position, "fixed" | "sticky")
      || position != "static"
        && style
          .get(Prop::Z_INDEX)
          .is_some_and(|value| value.parse::<i32>().is_ok())
      || masked
      || opacity(style) < 1.,
    absolute: fixed || will_position || position != "static",
    fixed,
    layer,
    transformed,
    preserves,
  }
}

// A `border-style` keyword as the renderer draws it.
pub(crate) fn border_style(kind: &str) -> renderer::BorderStyle {
  match kind {
    "dashed" => renderer::BorderStyle::Dashed,
    "dotted" => renderer::BorderStyle::Dotted,
    "double" => renderer::BorderStyle::Double,
    _ => renderer::BorderStyle::Solid,
  }
}

pub(crate) const TRANSFORM_PROPS: [Prop; 5] = [
  Prop::TRANSFORM,
  Prop::TRANSLATE,
  Prop::ROTATE,
  Prop::SCALE,
  Prop::OFFSET_PATH,
];

// Must list every property that `effects::opacity` and `crate::css::value::transform::transform` read.
pub(crate) const COMPOSITED_PROPS: [Prop; 6] = [
  Prop::OPACITY,
  Prop::TRANSFORM,
  Prop::TRANSLATE,
  Prop::ROTATE,
  Prop::SCALE,
  Prop::TRANSFORM_ORIGIN,
];

// The filter and the backdrop filter, in the order paint reads them.
pub(crate) const BLURS: [[Prop; 2]; 2] = [
  [Prop::FILTER, Prop::WEBKIT_FILTER],
  [Prop::BACKDROP_FILTER, Prop::WEBKIT_BACKDROP_FILTER],
];

/// Whether the compositor can play an animation of the property between page frames.
pub(crate) fn composited(prop: Prop) -> bool {
  COMPOSITED_PROPS.contains(&prop) || BLURS.as_flattened().contains(&prop)
}

/// The filter (0) or backdrop filter (1) of a style.
pub(crate) fn filter(style: &css::properties::Style, which: usize) -> Option<&str> {
  BLURS[which].iter().find_map(|&prop| style.get(prop))
}

/// The layer key of the `::before` (slot 1) or `::after` (slot 2) of a node. Element layers use the node id.
pub(crate) fn pseudo_key(id: usize, slot: u8) -> u64 {
  id as u64 | u64::from(slot) << 56
}

#[cfg(test)]
mod tests {
  use super::*;

  pub(super) fn style(pairs: &[(&str, &str)]) -> Style {
    pairs
      .iter()
      .map(|(key, value)| (crate::css::properties::Prop::new(key), (*value).into()))
      .collect()
  }

  pub(super) fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
  }

  #[test]
  fn transforms_masks_blends_and_containment_establish_contexts_and_blocks() {
    let of = |pairs: &[(&str, &str)]| establishes(&style(pairs));
    for prop in ["translate", "rotate", "scale", "transform"] {
      let value = if prop == "rotate" {
        "5deg"
      } else if prop == "transform" {
        "scale(2)"
      } else {
        "2px"
      };
      let made = of(&[(prop, value)]);
      assert!(
        made.stacking && made.absolute && made.fixed && made.transformed,
        "{prop}"
      );
      assert!(!of(&[(prop, "none")]).stacking, "{prop}");
    }
    for (prop, value) in [
      ("mask-image", "linear-gradient(red,blue)"),
      ("-webkit-mask-image", "url(a.png)"),
      ("mask", "url(#m)"),
      ("mix-blend-mode", "multiply"),
      ("isolation", "isolate"),
      ("clip-path", "circle()"),
      ("opacity", "0.5"),
      ("will-change", "opacity"),
      ("position", "sticky"),
    ] {
      let made = of(&[(prop, value)]);
      assert!(
        made.stacking && made.absolute == (prop == "position") && !made.fixed,
        "{prop}"
      );
    }
    for (prop, value) in [
      ("filter", "blur(2px)"),
      ("backdrop-filter", "blur(2px)"),
      ("perspective", "100px"),
      ("contain", "paint"),
      ("content-visibility", "auto"),
      ("will-change", "opacity, Transform"),
      ("transform-style", "preserve-3d"),
    ] {
      let made = of(&[(prop, value)]);
      assert!(made.stacking && made.absolute && made.fixed, "{prop}");
    }
    let relative = of(&[("position", "relative")]);
    assert!(relative.absolute && !relative.fixed && !relative.stacking);
    assert!(of(&[("position", "relative"), ("z-index", "1")]).stacking);
    assert!(!of(&[("position", "relative"), ("z-index", "auto")]).stacking);
    assert!(of(&[("will-change", "position")]).absolute);
    assert_eq!(
      of(&[("contain", "size"), ("container-type", "inline-size")]),
      Establishes::default()
    );
    assert_eq!(
      of(&[("display", "contents"), ("transform", "scale(2)")]),
      Establishes::default()
    );
    let preserves = |pair| of(&[("transform-style", "preserve-3d"), pair]).preserves;
    assert!(preserves(("contain", "layout")) && preserves(("will-change", "transform")));
    for pair in [
      ("transform-style", "flat"),
      ("opacity", "0.9"),
      ("overflow-x", "clip"),
      ("filter", "blur(0)"),
      ("clip-path", "inset(0)"),
      ("mask-image", "linear-gradient(red,blue)"),
      ("mix-blend-mode", "multiply"),
      ("isolation", "isolate"),
      ("contain", "layout paint"),
      ("will-change", "opacity"),
    ] {
      assert!(!preserves(pair), "{pair:?}");
    }
    assert!(of(&[("-webkit-filter", "blur(1px)")]).layer && !of(&[("mask", "url(#m)")]).layer);
  }
}
