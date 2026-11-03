//! h2o's windows and GPU renderer: Metal on macOS, Direct3D 11 on Windows, wgpu on Linux.
//!
//! Each frame the view builds a tree of elements, which is laid out with taffy, painted into a
//! [`Scene`], and drawn by the platform renderer. Windows draw only when something changed.

mod app;
mod atlas;
mod bounds_tree;
mod color;
mod div;
mod element;
mod frame;
mod geometry;
mod input;
mod layout;
mod platform;
mod scene;
mod style;
pub mod text;
mod window;

pub use app::{App, Timer, Waker, run};
pub use color::*;
pub use div::{Div, ScrollHandle, div};
pub use element::*;
pub use geometry::*;
pub use input::*;
pub use layout::{AvailableSpace, LayoutId};
pub use scene::{Backdrop, BlendMode, BorderStyle, PaintEffect, RenderImage, Scene};
pub use style::*;
pub use text::{
    DecorationStyle, Font, FontId, FontStyle, FontWeight, LineLayout, SUBPIXEL_VARIANTS_X,
    SharedString, StrikethroughStyle, StyledText, TextAlign, TextLayout, TextRun, TextSystem,
    TextWrap, UnderlineStyle, WhiteSpace, WordBreak, WrapBoundary, WrappedLineLayout, font,
};
pub use window::*;

pub mod prelude {
    pub use crate::{Element, FluentBuilder, IntoElement, ParentElement, Styled};
}
