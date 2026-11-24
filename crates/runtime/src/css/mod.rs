//! CSS syntax and values: stylesheets, declarations, properties, value grammar and selectors. It knows no
//! document, so the DOM and the style pass both build on it.

pub(crate) mod properties;
mod selector;
mod sheet;
mod value;

pub use properties::Style;
pub(crate) use properties::{BORDERS, LOGICAL, MARGIN, PADDING, Prop, PropSet, Registry, Vars};
pub(crate) use selector::{
    ACTIVE, Ancestry, BLOOM, Bloom, FOCUS, FOCUS_VISIBLE, FOCUS_WITHIN, HOVER, Selector,
    SelectorList, SimpleSelector, feature, lowercase,
};
pub(crate) use sheet::parser::supports_condition;
pub(crate) use sheet::parser::{DROPPED, FONT_FORMATS, FONT_TECHS, invalid};
pub(crate) use sheet::{
    Declaration, Declarations, Declared, Edit, MediaQuery, Rule, Stylesheet, declarations, declare,
};
pub use value::color::{color, p3, paint, rgba, wide, wide_rgb};
pub(crate) use value::color::{oklab, serialize as serialize_color, system as system_color};
pub(crate) use value::counter::counter_text;
pub(crate) use value::easing::{Easing, time, timing_function};
pub(crate) use value::grammar::{
    INITIAL, SHORTHANDS, computed, computed_names, fold, supports, syntax_matches,
};
pub use value::length;
pub(crate) use value::resolved::{percent_sum, percent_text, physical_name, resolved};
pub(crate) use value::shorthand::{BORDER_IMAGE, BORDER_IMAGE_INITIAL, border_image, expand};
pub(crate) use value::specified::shortest;
pub(crate) use value::specified::specified;
pub(crate) use value::tokens::{components, number, write_number, write_printed};
pub(crate) use value::transform::{
    IDENTITY, Matrix, angle, multiply, origin, transform, translate,
};
pub(crate) use value::units::{FontUnits, Kind, Lengths};
pub(crate) use value::{Filter, area_cells, box_shadows, clip_path, corner_shape};
