//! h2o's windows and GPU renderer: Metal on macOS, Direct3D 11 on Windows, wgpu on Linux.
//!
//! The view keeps a tree of boxes in each window. A frame commits the boxes that changed, lays
//! out the ones whose inputs changed with taffy, paints them into a [`Scene`] in CSS paint order,
//! and draws it with the platform renderer. Windows draw only when something changed.

mod app;
mod atlas;
mod bounds_tree;
mod color;
mod frame;
mod geometry;
mod input;
mod node;
mod platform;
mod scene;
mod style;
pub mod text;
mod tree;
mod window;

pub use app::{App, THREAD_PER_APP, Timer, Waker, native, run, spawn};
pub use color::*;
pub use geometry::*;
pub use input::*;
pub use node::{
    AvailableSpace, Child, ElementId, Hooks, MeasureFn, Node, NodeCx, ScrollHandle, canvas, img,
    layers, node, text,
};
pub use scene::{Backdrop, BlendMode, BorderStyle, PaintEffect, RenderImage, Scene};
pub use style::*;
pub use text::{
    DecorationStyle, Font, FontId, FontStyle, FontWeight, LineLayout, SUBPIXEL_VARIANTS_X,
    SharedString, StrikethroughStyle, TextAlign, TextLayout, TextRun, TextSystem, TextWrap,
    UnderlineStyle, WhiteSpace, WordBreak, WrapBoundary, WrappedLineLayout, font,
};
pub use tree::Tree;
pub use window::*;
