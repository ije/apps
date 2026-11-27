use crate::css::{
  properties::Prop, properties::Style, properties::Vars, sheet::MediaQuery, value::units::Lengths,
};
use crate::style::vars::var;
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::rc::Rc;
#[derive(PartialEq, Eq)]
pub(crate) struct Container {
  pub(crate) id: usize,
  // `container-type: normal` with a name, for named `style()` queries only.
  pub(crate) sized: bool,
  pub(crate) size: bool,
  pub(crate) names: Option<Rc<str>>,
  // Custom properties of a named container, only while a sheet has named `style()` queries.
  pub(crate) vars: Option<Vars>,
  // Queries in `em` depend on the font size.
  pub(crate) font: Option<Rc<str>>,
  pub(crate) up: Option<Rc<Container>>,
}

impl Container {
  pub(crate) fn of(
    id: usize,
    style: &Style,
    vars: Option<&Vars>,
    up: Option<Rc<Container>>,
  ) -> Option<Self> {
    let kind = style.get(Prop::CONTAINER_TYPE).unwrap_or("normal");
    let sized = matches!(kind, "size" | "inline-size");
    let names = style
      .value(Prop::CONTAINER_NAME)
      .filter(|names| &***names != "none")
      .cloned();
    (sized || names.is_some()).then(|| Self {
      id,
      sized,
      size: kind == "size",
      vars: vars.filter(|_| names.is_some()).cloned(),
      names,
      font: sized
        .then(|| style.value(Prop::FONT_SIZE).cloned())
        .flatten(),
      up,
    })
  }
}

// A `style()` query of custom properties against `vars`, the parent's, which adds the names it reads to
// `names`. `None` for other queries.
pub(crate) fn style_query(
  query: &MediaQuery,
  vars: Option<&Vars>,
  names: &mut Vec<Prop>,
) -> Option<bool> {
  match query {
    MediaQuery::Style(name, value) => {
      // Interned, so a name that no rule declares is still read.
      let prop = Prop::new(name);
      names.push(prop);
      let found = vars.and_then(|vars| var(vars, prop));
      Some(found.is_some_and(|found| value.is_empty() || found.trim() == value))
    }
    MediaQuery::Not(query) => style_query(query, vars, names).map(|matched| !matched),
    MediaQuery::And(items) | MediaQuery::Or(items) => {
      let and = matches!(query, MediaQuery::And(_));
      let mut result = and;
      for item in items {
        if style_query(item, vars, names)? != and {
          result = !and;
        }
      }
      Some(result)
    }
    _ => None,
  }
}

pub(crate) fn query_containers(
  conditions: &[(Option<String>, bool, MediaQuery)],
  chain: Option<&Container>,
  (scope, order): (usize, usize),
  sizes: &FxHashMap<usize, [f32; 2]>,
  lengths: impl Fn(usize) -> Lengths,
  queries: &mut QueryCache,
  (vars, names): (Option<&Vars>, &std::cell::RefCell<Vec<Prop>>),
) -> bool {
  conditions
    .iter()
    .enumerate()
    .all(|(index, (name, block, query))| {
      let named = |container: &Container| {
        name.as_ref().is_none_or(|name| {
          container
            .names
            .as_ref()
            .is_some_and(|names| names.split_ascii_whitespace().any(|item| item == name))
        })
      };
      let mut link = chain;
      // A named style query reads the nearest container of that name, of any type. The chain holds its
      // values, so a change gives a new chain and needs no reads.
      let mut names = names.borrow_mut();
      if name.is_some() {
        let mut found = chain;
        while let Some(container) = found.filter(|container| !named(container)) {
          found = container.up.as_deref();
        }
        let Some(container) = found else {
          return false;
        };
        let read = names.len();
        let matched = style_query(query, container.vars.as_ref(), &mut names);
        names.truncate(read);
        if let Some(matched) = matched {
          return matched;
        }
      } else if let Some(matched) = style_query(query, vars, &mut names) {
        return matched;
      }
      drop(names);
      while let Some(container) = link {
        if container.sized && (!block || container.size) && named(container) {
          let lengths = lengths(container.id);
          let matched = sizes.get(&container.id).is_some_and(|size| {
            query.matches(
              size[0],
              if container.size { size[1] } else { f32::NAN },
              false,
              0,
              lengths,
            ) == Some(true)
          });
          queries.insert(
            (scope, order, index, container.id),
            (!container.size, matched, lengths),
          );
          return matched;
        }
        link = container.up.as_deref();
      }
      false
    })
}

pub(crate) type QueryCache = HashMap<(usize, usize, usize, usize), (bool, bool, Lengths)>;

pub(crate) type ContainerQuery = Rc<[(Option<String>, bool, MediaQuery)]>;
