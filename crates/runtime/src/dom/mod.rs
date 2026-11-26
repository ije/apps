mod document;
pub(crate) mod geometry;
mod html;
mod interaction;
mod mirror;
mod selector;
pub(crate) mod text;
pub(crate) mod tree;
mod walk;

pub use document::{Document, Node, escape};
pub use geometry::Geometry;
pub use interaction::Interaction;
pub use mirror::LayoutMirror;
pub use selector::matches;
pub(crate) use selector::{Dependency, SelectorCache};
pub use tree::Tree;
pub(crate) use walk::{ABSENT, Walk};

pub(crate) const CHECKED: u32 = 16;
pub(crate) const DIRTY: u32 = 32;
pub(crate) const MODAL: u32 = 64;
pub(crate) const POPOVER: u32 = 256;
pub(crate) const FULLSCREEN: u32 = 512;
pub(crate) const BROKEN: u32 = 1024;
// Warning: one bit for both, as only custom elements can be undefined and they are never inputs.
pub(crate) const INDETERMINATE: u32 = 4096;
pub(crate) const DEFINED: u32 = INDETERMINATE;
pub(crate) const INVALID: u32 = 32768;
// The user changed the control, or a submit of its form checked it.
pub(crate) const USER: u32 = 1 << 16;
// A form-associated custom element.
pub(crate) const FORM_ASSOCIATED: u32 = 1 << 17;
pub(crate) const TOP_LAYER: u32 = MODAL | POPOVER | FULLSCREEN;
pub(crate) const LEAVING: u32 = TOP_LAYER << 5;

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
