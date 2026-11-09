mod app;
mod atlas;
mod bounds_tree;
mod color;
mod control;
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

pub use app::{App, THREAD_PER_APP, Timer, native, run, spawn};
pub use color::*;
pub use control::*;
pub use geometry::*;
pub use input::*;
pub use node::{
    AvailableSpace, Child, ElementId, Hooks, Node, NodeCx, ScrollHandle, canvas, canvas_within,
    layers, node, shared, text,
};
pub use scene::{Backdrop, BlendMode, BorderStyle, Glass, PaintEffect, RenderImage, Scene};
pub use style::*;
pub use text::{
    DecorationStyle, Font, FontId, FontStyle, FontWeight, LineLayout, SUBPIXEL_VARIANTS_X,
    SharedString, StrikethroughStyle, TextAlign, TextLayout, TextRun, TextSystem, TextWrap,
    UnderlineStyle, WhiteSpace, WordBreak, WrapBoundary, WrappedLineLayout, font,
};
pub use tree::Tree;
pub use window::*;

pub fn supports_glass() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform::supports_glass()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
