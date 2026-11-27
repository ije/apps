// Origin values are in precedence order and important ones rank `IMPORTANT - origin`, so don't reorder them.
use crate::css::{properties::LOGICAL, properties::Prop, properties::Style, properties::side};
use crate::style::cascade::shadow::ShadowRule;
use bbtree::unsync::BBTreeMap;
pub(crate) const AGENT: u8 = 0;
pub(crate) const USER: u8 = 1;
pub(crate) const HINT: u8 = 2;
pub(crate) const AUTHOR: u8 = 3;
pub(crate) const ANIMATION: u8 = 4;
pub(crate) const IMPORTANT: u8 = 8;

// The derived `Ord` compares fields in declaration order, which must stay the cascade's order.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CascadeRank {
  pub(crate) level: u8,
  pub(crate) context: i8,
  pub(crate) inline: bool,
  pub(crate) layer: u32,
  pub(crate) specificity: [u32; 3],
  pub(crate) order: u128,
  pub(crate) position: u16,
}

impl CascadeRank {
  // The rank of a declaration of a rule from another tree: important ones turn the order of the trees around.
  pub(crate) fn shadowed(rule: &ShadowRule, important: bool, position: u16) -> Self {
    Self {
      context: if important {
        -rule.context
      } else {
        rule.context
      },
      ..Self::rule(
        rule.layer,
        important,
        rule.specificity,
        rule.order,
        position,
      )
    }
  }

  pub(crate) fn new(
    origin: u8,
    important: bool,
    layer: u32,
    specificity: [usize; 3],
    order: u128,
  ) -> Self {
    Self {
      level: if important {
        IMPORTANT - origin
      } else {
        origin
      },
      context: 0,
      inline: false,
      layer: if important { !layer } else { layer },
      specificity: specificity.map(|value| value.min(u32::MAX as usize) as u32),
      order,
      position: 0,
    }
  }

  pub(crate) fn rule(
    layer: Layer,
    important: bool,
    specificity: [usize; 3],
    order: u128,
    position: u16,
  ) -> Self {
    Self {
      position,
      ..Self::new(layer.origin, important, layer.rank, specificity, order)
    }
  }

  pub(crate) fn important(&self) -> bool {
    self.level > ANIMATION
  }

  pub(crate) fn source(&self) -> (u8, u32) {
    let origin = if self.important() {
      IMPORTANT - self.level
    } else {
      self.level
    };
    (
      origin.min(HINT),
      if self.important() {
        !self.layer
      } else {
        self.layer
      },
    )
  }
}

// On equal ranks the later declaration wins.
pub(crate) fn put<V>(
  cascade: &mut BBTreeMap<Prop, (CascadeRank, V)>,
  prop: Prop,
  rank: CascadeRank,
  value: V,
) {
  match cascade.entry(prop) {
    bbtree::unsync::bbtree_map::Entry::Occupied(mut old) => {
      if rank >= old.get().0 {
        old.insert((rank, value));
      }
    }
    bbtree::unsync::bbtree_map::Entry::Vacant(slot) => {
      slot.insert((rank, value));
    }
  }
}

pub(crate) fn physical<V: AsRef<str> + Clone>(
  cascade: &mut BBTreeMap<Prop, (CascadeRank, V)>,
  parent: &Style,
) {
  let vertical = |mode: &str| mode.starts_with("vertical") || mode.starts_with("sideways");
  let lines = cascade
    .get(&Prop::WRITING_MODE)
    .filter(|(_, mode)| {
      vertical(mode.as_ref()) && !parent.get(Prop::WRITING_MODE).is_some_and(vertical)
    })
    .cloned();
  if cascade.keys().any(|prop| prop.is_logical()) {
    let rtl = match cascade.get(&Prop::DIRECTION).map(|value| value.1.as_ref()) {
      Some("rtl") => true,
      Some("ltr") => false,
      _ => parent.get(Prop::DIRECTION) == Some("rtl"),
    };
    let mode = lines.as_ref().map(|(_, mode)| mode.as_ref());
    for (logical, ..) in LOGICAL {
      if let Some((rank, value)) = cascade.remove(&logical) {
        put(cascade, side(logical, rtl, mode), rank, value);
      }
    }
  }
  if let Some((rank, mode)) = lines {
    put(cascade, Prop::INTERNAL_LINES, rank, mode);
  }
}
// While rules are collected, `rank` is the index of the layer in `Collect.layers`, not its rank.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Layer {
  pub(crate) origin: u8,
  pub(crate) rank: u32,
}

pub(crate) const DELETED: u8 = u8::MAX;
pub(crate) const UNLAYERED: u32 = u32::MAX;
