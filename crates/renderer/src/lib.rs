mod app;
mod atlas;
mod color;
mod control;
mod frame;
mod geometry;
mod gpu;
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
    AvailableSpace, Child, Hooks, Node, NodeCx, NodeKey, ScrollRef, canvas, canvas_within, layers,
    node, shared, text,
};
pub(crate) use scene::Scene;
pub use scene::{Backdrop, BlendMode, BorderStyle, Glass, PaintEffect, RenderImage};
pub use style::*;
pub use text::{
    Decoration, DecorationStyle, Font, FontId, FontStyle, FontWeight, Glyph, GlyphId, Line,
    Paragraph, SUBPIXEL_STEPS, SharedString, TextAlign, TextLine, TextOverrides, TextRun,
    TextStyle, TextSystem, TextWrap, WhiteSpace, WordBreak, Wrapping, widest_piece,
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
