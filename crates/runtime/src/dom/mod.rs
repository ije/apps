mod document;
mod entities;
pub(crate) mod geometry;
mod interaction;
mod mirror;
mod selector;
pub(crate) mod text;
pub(crate) mod tree;
mod walk;

pub use document::{Document, Node, escape};
pub(crate) use geometry::Drawn;
pub use geometry::Geometry;
pub use interaction::Interaction;
pub use mirror::LayoutMirror;
pub use selector::matches;
pub(crate) use selector::{Dependency, SelectorCache};
pub use tree::Tree;
pub(crate) use walk::{ABSENT, Walk};

pub(crate) const CHECKED: u16 = 16;
pub(crate) const DIRTY: u16 = 32;
pub(crate) const MODAL: u16 = 64;
pub(crate) const POPOVER: u16 = 256;
pub(crate) const FULLSCREEN: u16 = 512;
pub(crate) const BROKEN: u16 = 1024;
// Warning: one bit for both, as only custom elements can be undefined and they are never inputs.
pub(crate) const INDETERMINATE: u16 = 4096;
pub(crate) const DEFINED: u16 = INDETERMINATE;
pub(crate) const INVALID: u16 = 32768;
pub(crate) const TOP_LAYER: u16 = MODAL | POPOVER | FULLSCREEN;
pub(crate) const LEAVING: u16 = TOP_LAYER << 5;

// A `contenteditable` value makes the element editable or not, or inherits when `None`.
pub(crate) fn editable_value(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        (value.is_empty()
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("plaintext-only"))
        .then_some(true)
    }
}
