use crate::css::{
  properties::Prop, properties::Style, properties::Vars, value::units::FontUnits,
  value::units::Lengths,
};
use crate::style::cascade::Container;
use crate::style::cascade::computed::Extras;
use crate::style::cascade::shadow::ShadowRule;
use std::rc::Rc;
pub(crate) struct StyleRef<T: ?Sized = Style>(pub(crate) Rc<T>);

impl<T: ?Sized> Clone for StyleRef<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}
impl<T: ?Sized> PartialEq for StyleRef<T> {
  fn eq(&self, other: &Self) -> bool {
    Rc::ptr_eq(&self.0, &other.0)
  }
}
impl<T: ?Sized> Eq for StyleRef<T> {}
impl<T: ?Sized> std::hash::Hash for StyleRef<T> {
  fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
    std::ptr::hash(Rc::as_ptr(&self.0) as *const (), state);
  }
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(crate) struct Inputs {
  pub(crate) parent: Option<(StyleRef, bool)>,
  pub(crate) units: [u32; 16],
}

impl Inputs {
  pub(crate) fn new(font: FontUnits, root: FontUnits) -> Self {
    let mut units = [0; 16];
    units[..8].copy_from_slice(&font.bits());
    units[8..].copy_from_slice(&root.bits());
    Self {
      parent: None,
      units,
    }
  }

  pub(crate) fn hash(&self) -> u64 {
    self.units.iter().fold(0, |hash: u64, &bits| {
      (hash.rotate_left(5) ^ u64::from(bits)).wrapping_mul(0x517c_c1b7_2722_0a95)
    })
  }
}

#[derive(Hash, PartialEq, Eq)]
pub(crate) struct StyleKey {
  pub(crate) scope: usize,
  pub(crate) tag: u32,
  // Warning: the cascade resets native controls, so inputs that match the same rules can differ.
  pub(crate) control: Option<renderer::ControlKind>,
  pub(crate) inline: Option<u64>,
  pub(crate) hints: Vec<(Prop, Rc<str>)>,
  pub(crate) inputs: Inputs,
  pub(crate) rules: StyleRef<[(usize, [usize; 3])]>,
  pub(crate) pseudos: Rc<[usize]>,
  pub(crate) shadow: Option<Rc<[ShadowRule]>>,
  pub(crate) selection: Option<StyleRef>,
}

#[derive(PartialEq)]
pub(crate) struct RareInputs {
  pub(crate) shadow: Option<Rc<[ShadowRule]>>,
  pub(crate) selection: Option<StyleRef>,
}

pub(crate) struct CachedNode {
  pub(crate) parent: Option<StyleRef>,
  pub(crate) units: u64,
  pub(crate) inherited: Option<Vars>,
  pub(crate) vars: Vars,
  pub(crate) containers: Option<Option<Rc<Container>>>,
  pub(crate) rare: Option<Box<RareInputs>>,
  pub(crate) value: Rc<CachedStyle>,
}

impl CachedNode {
  pub(crate) fn new(
    key: &StyleKey,
    parent: Option<&Rc<Style>>,
    value: Rc<CachedStyle>,
    inherited: Option<&Vars>,
    vars: &Vars,
    containers: Option<Option<Rc<Container>>>,
  ) -> Self {
    let rare = (key.shadow.is_some() || key.selection.is_some()).then(|| {
      Box::new(RareInputs {
        shadow: key.shadow.clone(),
        selection: key.selection.clone(),
      })
    });
    Self {
      parent: parent.cloned().map(StyleRef),
      units: key.inputs.hash(),
      inherited: inherited.cloned(),
      vars: vars.clone(),
      containers,
      rare,
      value,
    }
  }

  pub(crate) fn same_rare(
    &self,
    shadow: Option<&Rc<[ShadowRule]>>,
    selection: Option<&Rc<Style>>,
  ) -> bool {
    match &self.rare {
      None => shadow.is_none() && selection.is_none(),
      Some(rare) => {
        rare.shadow.as_ref() == shadow
          && match (&rare.selection, selection) {
            (Some(old), Some(new)) => Rc::ptr_eq(&old.0, new),
            (old, new) => old.is_none() && new.is_none(),
          }
      }
    }
  }
}

pub(crate) type Read = (Prop, Option<Rc<str>>);

pub(crate) struct CachedStyle {
  pub(crate) used: std::cell::Cell<u64>,
  pub(crate) inline: Option<Box<str>>,
  pub(crate) style: Rc<Style>,
  pub(crate) vars: std::cell::RefCell<(Option<Vars>, Vars)>,
  pub(crate) reads: Option<Box<[Read]>>,
  pub(crate) own: Box<[Read]>,
  pub(crate) missing: Box<[Box<str>]>,
  pub(crate) flags: u16,
  pub(crate) lengths: Lengths,
  pub(crate) extras: Option<Rc<Extras>>,
  pub(crate) viewport: bool,
  pub(crate) keyword: bool,
  // Anchors place boxes with it.
  pub(crate) positions: bool,
}

pub(crate) struct Share {
  pub(crate) from: usize,
  pub(crate) rules: Rc<[(usize, [usize; 3])]>,
  pub(crate) pseudos: Rc<[usize]>,
  pub(crate) dependencies: Box<[crate::dom::Dependency]>,
  pub(crate) style: Option<u32>,
}

pub(crate) type Inheritable = (Vars, Option<Vars>);
