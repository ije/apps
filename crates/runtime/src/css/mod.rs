//! CSS syntax and values: stylesheets, declarations, properties, value grammar and selectors. It knows no
//! document, so the DOM and the style pass both build on it.

pub mod properties;
pub(crate) mod selector;
pub(crate) mod sheet;
pub mod value;
