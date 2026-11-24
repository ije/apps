mod app;
mod color;
mod control;
mod geometry;
mod gpu;
mod input;
mod node;
mod platform;
mod style;
pub mod text;
mod tree;
mod window;

pub use app::{App, THREAD_PER_APP, run, spawn};
pub use color::*;
pub use control::*;
pub use geometry::*;
pub use gpu::Snapshot;
pub(crate) use gpu::scene::Scene;
pub use gpu::scene::{Backdrop, BlendMode, Glass, PaintEffect, RenderImage};
pub use input::*;
pub use node::{
    Child, Hooks, Node, NodeCx, NodeKey, ScrollRef, canvas, canvas_watching, canvas_within, layers,
    node, shared, text,
};
pub use platform::{
    CARET_BLINK, Timer, WindowChange, WindowControl, WindowId, WindowOptions, WindowState,
    control_size, native,
};
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
