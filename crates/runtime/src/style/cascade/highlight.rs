use super::StyleCache;
use super::computed::ComputedStyles;
use super::rank::{CascadeRank, put};
use super::shadow::HIGHLIGHT;
use crate::css::{properties::Prop, properties::Style};
use crate::style::vars::resolve_style;
use bbtree::unsync::BBTreeMap;
use rustc_hash::FxHashMap;
use std::rc::Rc;

// The `::highlight()` styles that paint read since the last style pass, by element and name.
#[derive(Default)]
pub(crate) struct Highlights {
  pass: u64,
  styles: FxHashMap<(usize, Rc<str>), Option<Rc<Style>>>,
}

const PAINTED: [Prop; 8] = [
  Prop::COLOR,
  Prop::BACKGROUND_COLOR,
  Prop::WEBKIT_TEXT_FILL_COLOR,
  Prop::TEXT_DECORATION_LINE,
  Prop::TEXT_DECORATION_COLOR,
  Prop::TEXT_DECORATION_STYLE,
  Prop::TEXT_DECORATION_THICKNESS,
  Prop::TEXT_UNDERLINE_OFFSET,
];

impl StyleCache {
  // The style of a `::highlight()` for the text of element `id`. It inherits from the highlight of the parent
  // element. Style passes only match the rules, so pages without highlights pay nothing for them.
  pub(crate) fn highlight(
    &self,
    computed: &ComputedStyles,
    kept: &mut Highlights,
    id: usize,
    name: &Rc<str>,
  ) -> Option<Rc<Style>> {
    if kept.pass != self.pass {
      kept.styles.clear();
      kept.pass = self.pass;
    }
    let walk = self.walk.as_ref()?;
    let (mut chain, mut at, mut style) = (Vec::new(), Some(id), None);
    while let Some(node) = at {
      if let Some(found) = kept.styles.get(&(node, name.clone())) {
        style = found.clone();
        break;
      }
      chain.push(node);
      at = walk.walk.parent(node).map(|parent| parent as usize);
      at = at.filter(|&parent| parent != crate::dom::ABSENT as usize);
    }
    while let Some(node) = chain.pop() {
      style = self.own_highlight(computed, node, name, style);
      kept.styles.insert((node, name.clone()), style.clone());
    }
    style
  }

  fn own_highlight(
    &self,
    computed: &ComputedStyles,
    id: usize,
    name: &Rc<str>,
    parent: Option<Rc<Style>>,
  ) -> Option<Rc<Style>> {
    let Some(scope) = self.styled.as_ref().and_then(|styled| {
      let root = styled.trees.get(*styled.tree.get(id)? as usize)?.0;
      let at = styled.scopes.binary_search_by_key(&root, |scope| scope.0);
      let scope = styled.scopes[at.ok()?].1;
      self.scopes.iter().find(|own| own.id == scope)
    }) else {
      return parent;
    };
    let mut cascade = BBTreeMap::new();
    let matched = scope.nodes.get(id).and_then(Option::as_ref);
    for &index in matched.iter().flat_map(|matched| matched.pseudos.iter()) {
      let (kind, selector, order, _) = &scope.pseudos[index];
      if *kind != HIGHLIGHT
        || scope.containers[*order].is_some()
        || scope.arguments.get(&index).map(|found| &found.0) != Some(name)
        || scope.medias[*order]
          .as_ref()
          .is_some_and(|stack| stack.iter().any(|block| !block.matched.get()))
      {
        continue;
      }
      let (_, declarations, layer) = &scope.rules[*order];
      for &(prop, ref value, important, position) in declarations.iter() {
        if PAINTED.contains(&prop) {
          let key = scope.keys[*order];
          let rank = CascadeRank::rule(*layer, important, selector.specificity, key, position);
          put(&mut cascade, prop, rank, value);
        }
      }
    }
    if cascade.is_empty() {
      return parent;
    }
    let mut style = Style::new();
    style.set_vars(computed.vars[id].clone());
    if let Some(scheme) = computed.values[id].value(Prop::COLOR_SCHEME) {
      style.insert(Prop::COLOR_SCHEME, scheme.clone());
    }
    style.extend(
      cascade
        .into_iter()
        .map(|(prop, (_, value))| (prop, value.clone())),
    );
    let declared: Vec<Prop> = style.own().map(|(prop, _)| prop).collect();
    let (registry, theme) = (&computed.registry, computed.theme);
    resolve_style(&mut style, parent.as_deref(), None, &[], registry, theme);
    style.set_vars(super::computed::empty().1);
    style.remove(Prop::COLOR_SCHEME);
    for prop in PAINTED {
      match style.get(prop).filter(|_| declared.contains(&prop)) {
        None | Some("inherit" | "unset") => {
          style.remove(prop);
          if let Some(value) = parent.as_ref().and_then(|parent| parent.value(prop)) {
            style.insert(prop, value.clone());
          }
        }
        Some("initial" | "revert" | "revert-layer") => {
          style.remove(prop);
        }
        _ => {}
      }
    }
    (!style.is_empty()).then(|| Rc::new(style))
  }
}
