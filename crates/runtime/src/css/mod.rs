//! CSS syntax and values: stylesheets, declarations, properties, value grammar and selectors. It knows no
//! document, so the DOM and the style pass both build on it.

pub(crate) mod color;
mod counter;
mod easing;
mod grammar;
mod media;
mod parser;
mod properties;
mod resolved;
mod selector;
mod sheet;
mod shorthand;
mod specified;
mod tokens;
mod units;
mod value;

pub use color::{color, p3, paint, rgba, wide, wide_rgb};
pub(crate) use counter::counter_text;
pub(crate) use easing::{Easing, time, timing_function};
pub(crate) use grammar::{
    INITIAL, SHORTHANDS, computed, computed_names, fold, supports, syntax_matches,
};
pub(crate) use media::supports_condition;
pub(crate) use parser::{DROPPED, FONT_FORMATS, FONT_TECHS, invalid};
pub use properties::Style;
pub(crate) use properties::{LOGICAL, Prop, PropSet, Registry, Vars};
pub(crate) use resolved::{percent_sum, percent_text, physical_name, resolved, shortest};
pub(crate) use selector::{
    ACTIVE, Ancestry, BLOOM, Bloom, FOCUS, FOCUS_VISIBLE, FOCUS_WITHIN, HOVER, Selector,
    SelectorList, SimpleSelector, feature, lowercase,
};
pub(crate) use sheet::{
    Declaration, Declarations, Declared, Edit, MediaQuery, Rule, Stylesheet, declarations, declare,
};
pub(crate) use shorthand::{BORDER_IMAGE, BORDER_IMAGE_INITIAL, border_image, expand};
pub(crate) use specified::specified;
pub(crate) use tokens::{components, number, write_number, write_printed};
pub(crate) use units::{FontUnits, Kind, Lengths};
pub use value::length;
pub(crate) use value::{
    BORDERS, Filter, MARGIN, PADDING, area_cells, box_shadows, clip_path, corner_shape,
};
